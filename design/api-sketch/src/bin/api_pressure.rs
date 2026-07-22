//! A broad end-to-end pressure test for Samara's proposed consumer API.
//!
//! The example has two Components: `Counter` owns a count, while `Quota` owns
//! the authority to approve changes to it. An increment therefore crosses a
//! named Port as a correlated request before it can become state. Successful
//! increments also produce a typed persistence effect, and an `mpsc` source
//! plus a logical timer demonstrate the two subscription/command lifecycles.
//!
//! This is deliberately more than a “hello world.” It keeps the important
//! boundaries visible: Component values contain immutable wiring, Models hold
//! mutable behavioral state, `update` only changes state and declares intent,
//! and adapters perform world-facing work. `live_shape` and `controlled_shape`
//! assemble the same application while giving those intents different worlds.

#![allow(dead_code)]

use std::time::Duration;

use samara_api_sketch::prelude::*;

/// Stable identity shared by the input source, its binding, and subscription.
const INCREMENTS: &str = "counter/increments";

/// The particular quota dependency used by this Counter instance.
const QUOTA: &str = "counter/quota";

/// Provider-neutral vocabulary for quota operations.
///
/// The protocol is separate from `QuotaMsg`, so consumers depend on the service
/// contract rather than the provider Component's complete private message API.
struct QuotaProtocol;

impl Protocol for QuotaProtocol {
    type Inbound = QuotaInbound;
}

#[derive(Debug)]
/// Values that program assembly can route through a `QuotaProtocol` Port.
enum QuotaInbound {
    /// A typed request together with the inert token needed to return its reply.
    Reserve(Incoming<QuotaProtocol, Reserve>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Requests authority to add `amount` to a Counter.
struct Reserve {
    /// The proposed increment.
    amount: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// The domain result of evaluating a reservation request.
enum Reservation {
    /// The requested capacity was consumed on the requester's behalf.
    Granted { amount: u64 },

    /// No capacity was consumed because the request exceeded what remained.
    Denied { requested: u64, remaining: u64 },
}

impl Request<QuotaProtocol> for Reserve {
    type Reply = Reservation;

    fn into_inbound(self, reply_to: ReplyTo<Self::Reply>) -> QuotaInbound {
        // This conversion packages domain data with runtime-owned correlation
        // data. Neither requester nor provider needs to see a correlation ID.
        QuotaInbound::Reserve(Incoming::new(self, reply_to))
    }
}

#[derive(Debug, PartialEq, Eq)]
/// Mutable behavioral state owned exclusively by the Quota Component.
struct QuotaModel {
    /// Capacity that has not yet been granted.
    remaining: u64,
}

#[derive(Debug)]
/// Quota's private message vocabulary.
enum QuotaMsg {
    /// A provider-neutral protocol value routed here by program assembly.
    Protocol(QuotaInbound),
}

/// Immutable configuration for one Quota Component instance.
struct Quota {
    /// Capacity copied into the Model when the Component starts.
    initial: u64,
}

impl Component for Quota {
    type Model = QuotaModel;
    type Msg = QuotaMsg;

    fn init(&self) -> Init<Self::Model, Self::Msg> {
        Init::new(QuotaModel {
            remaining: self.initial,
        })
    }

    fn update(&self, model: &mut Self::Model, msg: Self::Msg) -> Cmd<Self::Msg> {
        match msg {
            QuotaMsg::Protocol(QuotaInbound::Reserve(incoming)) => {
                // This is an ordinary deterministic transition: decide from
                // current state, update owned state, and describe a reply. The
                // runtime—not this function—delivers that reply.
                let requested = incoming.request.amount;
                let reservation = if requested <= model.remaining {
                    model.remaining -= requested;
                    Reservation::Granted { amount: requested }
                } else {
                    Reservation::Denied {
                        requested,
                        remaining: model.remaining,
                    }
                };

                Cmd::reply(incoming.reply_to, reservation)
            }
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
/// All mutable application state owned by the Counter Component.
struct CounterModel {
    /// Sum of increments that Quota has granted.
    count: u64,

    /// Number of logical timer events observed.
    ticks: u64,

    /// Most recent terminal outcome from the quota request.
    last_reservation: Option<RequestOutcome<Reservation>>,

    /// Most recent terminal outcome from the persistence effect.
    last_save: Option<EffectEvent<(), PersistError>>,

    /// Whether the ongoing increment source ended normally.
    input_closed: bool,
}

#[derive(Debug)]
/// Every message that may drive a Counter state transition.
enum CounterMsg {
    /// A proposed increment, regardless of whether it came from a source or send.
    Increment(u64),

    /// The subscribed input source ended normally.
    InputClosed,

    /// A terminal outcome from one particular quota request.
    Reserved(RequestOutcome<Reservation>),

    /// A terminal outcome from persisting an approved count.
    Persisted(EffectEvent<(), PersistError>),

    /// One interval elapsed on the runtime's clock.
    Tick,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Typed intent to persist the latest approved count.
struct PersistCount {
    /// Snapshot to write; the adapter does not read Component state directly.
    value: u64,
}

impl Effect for PersistCount {
    type Output = ();
    type Error = PersistError;
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Domain-facing persistence failure returned by an adapter.
struct PersistError {
    /// Human-readable explanation suitable for state or diagnostics.
    message: String,
}

/// Immutable logical wiring for one Counter Component instance.
struct Counter {
    /// Descriptor for the ongoing external increment source.
    increments: MpscSource<u64>,

    /// Named dependency on any provider of the quota protocol.
    quota: Port<QuotaProtocol>,
}

impl Component for Counter {
    type Model = CounterModel;
    type Msg = CounterMsg;

    fn init(&self) -> Init<Self::Model, Self::Msg> {
        // `after` declares timer intent against the runtime's clock. It does
        // not sleep, acquire a Tokio handle, or perform work during init.
        Init::new(CounterModel::default())
            .with_cmd(Cmd::after(Duration::from_secs(1), CounterMsg::Tick))
    }

    fn update(&self, model: &mut Self::Model, msg: Self::Msg) -> Cmd<Self::Msg> {
        match msg {
            CounterMsg::Increment(amount) => {
                // The count deliberately remains unchanged until authority is
                // granted. `CounterMsg::Reserved` is the explicit continuation:
                // the runtime maps this request's eventual terminal event back
                // into the requester's ordinary message stream.
                Cmd::request(self.quota.clone(), Reserve { amount }, CounterMsg::Reserved)
            }
            CounterMsg::Reserved(event) => {
                // Request correlation is runtime machinery, but its meaningful
                // outcomes remain explicit data. Only a granted domain reply
                // authorizes the state change and consequent side effect.
                let cmd = match &event {
                    RequestOutcome::Replied(Reservation::Granted { amount }) => {
                        model.count += amount;
                        Cmd::effect(PersistCount { value: model.count }, CounterMsg::Persisted)
                    }
                    RequestOutcome::Replied(Reservation::Denied { .. })
                    | RequestOutcome::Failed(_)
                    | RequestOutcome::TimedOut
                    | RequestOutcome::Cancelled => Cmd::none(),
                };
                model.last_reservation = Some(event);
                cmd
            }
            CounterMsg::Persisted(result) => {
                model.last_save = Some(result);
                Cmd::none()
            }
            CounterMsg::InputClosed => {
                model.input_closed = true;
                Cmd::none()
            }
            CounterMsg::Tick => {
                model.ticks += 1;
                // Re-issuing the command makes repetition explicit and keeps
                // time controllable; no background interval mutates the Model.
                Cmd::after(Duration::from_secs(1), CounterMsg::Tick)
            }
        }
    }

    fn subscriptions(&self, _model: &Self::Model) -> Subscriptions<Self::Msg> {
        // Subscriptions describe ongoing demand. The stable ID lets the runtime
        // reconcile this desired source across transitions without exposing a
        // task, receiver, or cancellation handle to the Component.
        Subscriptions::one(Subscription::source(
            SubscriptionId::new(INCREMENTS),
            self.increments.clone(),
            |event| match event {
                SourceEvent::Item(by) => CounterMsg::Increment(by),
                SourceEvent::Ended => CounterMsg::InputClosed,
                SourceEvent::Failed(never) => match never {},
            },
        ))
    }
}

/// Live executor for `PersistCount` intents.
///
/// Operational resources would belong here (or in fields on this adapter), not
/// in `CounterModel` or `Counter::update`.
struct LivePersistence;

impl EffectAdapter<PersistCount> for LivePersistence {
    fn execute(
        &self,
        effect: PersistCount,
    ) -> BoxFuture<Result<<PersistCount as Effect>::Output, <PersistCount as Effect>::Error>> {
        Box::pin(async move {
            // Replace with a real persistence boundary. The adapter, not the
            // Component, owns the side effect.
            let _ = effect;
            Ok(())
        })
    }
}

/// Addresses retained by the application boundary after program assembly.
///
/// Component-to-Component dependencies use Ports above; these references are
/// for deliberate direct interaction and state observation at the runtime edge.
struct AppRefs {
    /// Address used to send to and inspect the Counter.
    counter: ComponentRef<Counter>,

    /// Address used to inspect Quota's independently owned state.
    quota: ComponentRef<Quota>,
}

/// Declares the logical application graph shared by every execution profile.
fn program(increments: MpscSource<u64>) -> (Program, AppRefs) {
    let mut program = Program::builder();

    // A named Port is immutable dependency wiring. Naming allows another Quota
    // protocol instance to coexist without a global “one provider per type” rule.
    let quota_port = program.port(PortId::new(QUOTA));
    let quota = program.component(ComponentId::new("quota"), Quota { initial: 100 });

    // Assembly chooses the concrete provider and the single translation into
    // its private Msg vocabulary; Counter never needs to know `QuotaMsg`.
    program.bind_port(&quota_port, &quota, QuotaMsg::Protocol);
    let counter = program.component(
        ComponentId::new("counter"),
        Counter {
            increments: increments.clone(),
            quota: quota_port,
        },
    );

    (program.build(), AppRefs { counter, quota })
}

/// Shows production-shaped execution with Tokio-backed world boundaries.
///
/// This function is compile-checked documentation in the API sketch. It uses
/// the same `program` as controlled execution, but binds real receivers and an
/// async effect adapter before spawning the live runtime.
async fn live_shape() -> Result<(), RuntimeError> {
    let increments = MpscSource::named(INCREMENTS);
    let (input, receiver) = tokio::sync::mpsc::channel(32);
    let (program, refs) = program(increments.clone());

    // Runtime assembly owns the operational channel receiver and effect
    // executor. The logical descriptors inside Components stay inert.
    let runtime = LiveRuntime::builder(program)
        .bind_mpsc(increments, receiver)
        .bind_effect::<PersistCount, _>(LivePersistence)
        .build()?;

    let counter = runtime.handle(&refs.counter)?;
    let runtime = runtime.spawn();

    // Both an adapted source and an explicit low-level send become CounterMsg
    // values. Neither path is allowed to mutate CounterModel directly.
    let _ = input.send(3).await;
    counter.send(CounterMsg::Increment(2)).await?;

    // Structured shutdown joins runtime-owned work and reports its outcome.
    let report = runtime.shutdown(Shutdown::Cancel).await?;
    assert!(report.is_clean());
    Ok(())
}

/// Shows deterministic execution against a runtime-controlled world.
///
/// Inputs, effect completion, and clock advancement happen only when the test
/// harness says so. Component code and logical assembly are unchanged from
/// `live_shape`; only runtime decisions and boundary bindings differ.
fn controlled_shape() -> Result<(), RuntimeError> {
    let increments = MpscSource::named(INCREMENTS);
    let (program, refs) = program(increments.clone());

    let mut runtime = ControlledRuntime::builder(program)
        .control_mpsc(increments.clone())
        .control_effect::<PersistCount>()
        .build()?;

    // This single controlled input drives the full causal chain:
    // Increment -> quota request -> Quota transition -> reply -> Counter transition.
    runtime.emit_mpsc(&increments, 3)?;
    runtime.run_until_idle()?;

    // The world-facing persistence intent is now pending, rather than having
    // executed invisibly. The harness chooses its terminal result and timing.
    let pending = runtime.next_effect::<PersistCount>()?;
    assert_eq!(pending.intent, PersistCount { value: 3 });
    runtime.complete(pending, EffectEvent::Succeeded(()))?;
    runtime.run_until_idle()?;

    // Advancing logical time deterministically delivers the scheduled Tick.
    runtime.advance(Duration::from_secs(1))?;

    // State inspection proves the result across both independently owned
    // Models: Counter accepted three and Quota consumed exactly three.
    let model = runtime.state(&refs.counter)?;
    assert_eq!(model.count, 3);
    assert_eq!(model.ticks, 1);
    assert_eq!(runtime.state(&refs.quota)?.remaining, 97);

    // A controlled run can expose its program-wide semantic trace without
    // requiring production execution to impose a global order on live events.
    let _semantic_trace = runtime.trace();

    // This scenario deliberately leaves its source open and its next Tick
    // scheduled. Consuming cancellation closes and accounts for both as
    // runtime-owned work; it does not claim the application itself completed.
    let report = runtime.cancel()?;
    assert!(report.is_clean());
    Ok(())
}

/// Keeps the binary intentionally inert while its consumer shape is compiled.
fn main() {
    println!("compile-only API sketch; see this source and its unit tests");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Proves pure transition intent without constructing either runtime.
    ///
    /// Runtime-level behavior belongs in controlled tests; this fast unit test
    /// checks the Component contract directly: a request does not prematurely
    /// mutate state, an injected grant produces state plus a typed effect, and
    /// the desired subscription remains inspectable data.
    #[test]
    fn transition_and_subscription_shape_are_directly_testable() {
        let increments = MpscSource::named(INCREMENTS);
        let mut program = Program::builder();
        let quota = program.port(PortId::new(QUOTA));
        let component = Counter {
            increments: increments.clone(),
            quota,
        };
        let mut model = component.init().model;

        // Dispatching a request declares intent only. The later reply is the sole
        // message that can authorize this change.
        let cmd = component.update(&mut model, CounterMsg::Increment(3));
        assert_eq!(model.count, 0);
        let requests = cmd.request_intents::<Reserve>();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, &PortId::new(QUOTA));
        assert_eq!(requests[0].1, &Reserve { amount: 3 });

        // Supplying the message shape produced by the continuation exercises
        // the resulting transition independently of transport correlation.
        let cmd = component.update(
            &mut model,
            CounterMsg::Reserved(RequestOutcome::Replied(Reservation::Granted { amount: 3 })),
        );
        assert_eq!(model.count, 3);
        assert_eq!(
            cmd.effect_intent::<PersistCount>(),
            Some(&PersistCount { value: 3 })
        );

        // Subscription configuration is declared and testable without running
        // the receiver or depending on a particular task/mailbox topology.
        let subscriptions = component.subscriptions(&model);
        assert_eq!(
            subscriptions.find::<MpscSource<u64>>(&SubscriptionId::new(INCREMENTS)),
            Some(&increments)
        );
    }
}
