//! A broad end-to-end pressure test for Samara's proposed consumer API.
//!
//! The example has two Components: `Counter` owns a count, while `Quota` owns
//! the authority to approve changes to it. An increment therefore crosses a
//! named Port as a correlated request before it can become state. Successful
//! increments also produce a typed persistence effect, and a logical stream
//! (`mpsc`-backed in live execution) plus a timer demonstrate the two
//! subscription/command lifecycles.
//!
//! This is deliberately more than a “hello world.” It keeps the important
//! boundaries visible: Component configurations contain immutable wiring,
//! Models hold mutable behavioral state, `update` only changes state and
//! declares intent, and Drivers perform world-facing work. `live_shape` and
//! `controlled_shape` assemble the same application while giving those intents
//! different worlds.

#![allow(dead_code)]

type AppProgram = Program<(Source<StreamDescriptor<u64>>, Effect<PersistCount>)>;

use std::time::Duration;

use samara::prelude::*;

/// Logical first-party input bound by the surrounding runtime world.
const INCREMENT_INPUT: &str = "counter/increment-input";

/// Component-local identity used to reconcile desire for the input.
const INCREMENT_SUBSCRIPTION: &str = "increments";

/// The particular quota dependency used by this Counter instance.
const QUOTA: &str = "counter/quota";

/// Provider-neutral vocabulary for quota operations.
///
/// This Port message enum is separate from `QuotaMessage`, so consumers depend
/// on the service contract rather than the provider Component's complete
/// private Message API. Program assembly routes these values through a
/// `Port<QuotaProtocolMessage>`.
#[derive(Debug)]
enum QuotaProtocolMessage {
    /// Requests authority to add `amount` to a Counter, together with the
    /// inert token needed to return the decision.
    Reserve {
        /// The proposed increment.
        amount: u64,
        /// Runtime-owned correlation; neither side sees a correlation ID.
        reply_with: ReplyWith<Reservation>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// The domain outcome of evaluating a reservation request.
enum Reservation {
    /// The requested capacity was consumed on the requester's behalf.
    Granted { amount: u64 },

    /// No capacity was consumed because the request exceeded what remained.
    Denied { requested: u64, remaining: u64 },
}

#[derive(Debug, PartialEq, Eq)]
/// Mutable behavioral state owned exclusively by the Quota Component.
struct QuotaModel {
    /// Capacity that has not yet been granted.
    remaining: u64,
}

#[derive(Debug)]
/// Quota's private message vocabulary.
enum QuotaMessage {
    /// A provider-neutral Port message routed here by program assembly.
    Protocol(QuotaProtocolMessage),
}

impl From<QuotaProtocolMessage> for QuotaMessage {
    fn from(message: QuotaProtocolMessage) -> Self {
        Self::Protocol(message)
    }
}

/// Immutable configuration for one Quota Component instance.
struct Quota;

struct QuotaConfig {
    /// Capacity copied into the Model when the Component starts.
    initial: u64,
}

impl Component for Quota {
    type Requires = ();
    type Config = QuotaConfig;
    type Model = QuotaModel;
    type Message = QuotaMessage;

    fn init(
        config: &Self::Config,
        _: &Capabilities<Self::Requires>,
    ) -> Init<Self::Model, Self::Message> {
        Init::new(QuotaModel {
            remaining: config.initial,
        })
    }

    fn update(
        _config: &Self::Config,
        _: &Capabilities<Self::Requires>,
        model: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        match message {
            QuotaMessage::Protocol(QuotaProtocolMessage::Reserve {
                amount: requested,
                reply_with,
            }) => {
                // This is an ordinary deterministic transition: decide from
                // current state, update owned state, and describe a reply. The
                // runtime—not this function—delivers that reply.
                let reservation = if requested <= model.remaining {
                    model.remaining -= requested;
                    Reservation::Granted { amount: requested }
                } else {
                    Reservation::Denied {
                        requested,
                        remaining: model.remaining,
                    }
                };

                Command::reply(reply_with, reservation)
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
    last_save: Option<EffectOutcome<(), PersistError>>,

    /// Whether the ongoing increment source ended normally.
    input_closed: bool,
}

#[derive(Debug)]
/// Every message that may drive a Counter state transition.
enum CounterMessage {
    /// A proposed increment, regardless of whether it came from a source or send.
    Increment(u64),

    /// The subscribed input source ended normally.
    InputClosed,

    /// A terminal outcome from one particular quota request.
    Reserved(RequestOutcome<Reservation>),

    /// A terminal outcome from persisting an approved count.
    Persisted(EffectOutcome<(), PersistError>),

    /// One interval elapsed on the runtime's clock.
    Tick,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Typed effect descriptor for persisting the latest approved count.
struct PersistCount {
    /// Snapshot to write; the Driver does not read Component state directly.
    value: u64,
}

impl EffectDescriptor for PersistCount {
    type Output = ();
    type Error = PersistError;
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Domain-facing persistence error data returned by a Driver.
struct PersistError {
    /// Human-readable explanation suitable for state or diagnostics.
    message: String,
}

/// Immutable logical wiring for one Counter Component instance.
struct Counter;

struct CounterConfig {
    /// Declared authority for the ongoing increment source.

    /// Descriptor for the ongoing external increment source.
    increments: StreamDescriptor<u64>,

    /// Declared authority for finite persistence effects.

    /// Named dependency on any provider of the quota message enum.
    quota: Port<QuotaProtocolMessage>,
}

impl Component for Counter {
    type Requires = (Source<StreamDescriptor<u64>>, Effect<PersistCount>);
    type Config = CounterConfig;
    type Model = CounterModel;
    type Message = CounterMessage;

    fn init(
        _config: &Self::Config,
        _capabilities: &Capabilities<Self::Requires>,
    ) -> Init<Self::Model, Self::Message> {
        // `after` declares timer intent against the runtime's clock. It does
        // not sleep, acquire a Tokio handle, or perform work during init.
        Init::new(CounterModel::default())
            .with_command(Command::after(Duration::from_secs(1), CounterMessage::Tick))
    }

    fn update(
        config: &Self::Config,
        capabilities: &Capabilities<Self::Requires>,
        model: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        match message {
            CounterMessage::Increment(amount) => {
                // The count deliberately remains unchanged until authority is
                // granted. `CounterMessage::Reserved` is the explicit continuation:
                // the runtime maps this request's eventual terminal outcome back
                // into the requester's ordinary message stream.
                Command::request_to(
                    &config.quota,
                    move |reply_with| QuotaProtocolMessage::Reserve { amount, reply_with },
                    CounterMessage::Reserved,
                )
            }
            CounterMessage::Reserved(outcome) => {
                // Request correlation is runtime machinery, but its meaningful
                // outcomes remain explicit data. Only a granted domain reply
                // authorizes the state change and consequent side effect.
                let command = match &outcome {
                    RequestOutcome::Replied(Reservation::Granted { amount }) => {
                        model.count += amount;
                        Command::effect_with(
                            &capabilities.1,
                            PersistCount { value: model.count },
                            CounterMessage::Persisted,
                        )
                    }
                    RequestOutcome::Replied(Reservation::Denied { .. })
                    | RequestOutcome::Failed(_)
                    | RequestOutcome::TimedOut
                    | RequestOutcome::Cancelled => Command::none(),
                };
                model.last_reservation = Some(outcome);
                command
            }
            CounterMessage::Persisted(outcome) => {
                model.last_save = Some(outcome);
                Command::none()
            }
            CounterMessage::InputClosed => {
                model.input_closed = true;
                Command::none()
            }
            CounterMessage::Tick => {
                model.ticks += 1;
                // Re-issuing the command makes repetition explicit and keeps
                // time controllable; no background interval mutates the Model.
                Command::after(Duration::from_secs(1), CounterMessage::Tick)
            }
        }
    }

    fn subscriptions(
        config: &Self::Config,
        capabilities: &Capabilities<Self::Requires>,
        _model: &Self::Model,
    ) -> Subscriptions<Self::Message> {
        // Subscriptions describe ongoing demand. The stable ID lets the runtime
        // reconcile this desired source across transitions without exposing a
        // task, receiver, or cancellation handle to the Component.
        Subscriptions::one(Subscription::source_with(
            &capabilities.0,
            SubscriptionId::new(INCREMENT_SUBSCRIPTION),
            config.increments.clone(),
            |event| match event {
                SourceEvent::Item(by) => CounterMessage::Increment(by),
                SourceEvent::Ended => CounterMessage::InputClosed,
                SourceEvent::Failed(never) => match never {},
            },
        ))
    }
}

/// Live Driver for `PersistCount` descriptors.
///
/// Operational resources would belong here (or in fields on this Driver), not
/// in `CounterModel` or `Counter::update`.
struct LivePersistence;

impl EffectDriver<PersistCount> for LivePersistence {
    fn execute(
        &self,
        descriptor: PersistCount,
    ) -> BoxFuture<
        Result<
            <PersistCount as EffectDescriptor>::Output,
            <PersistCount as EffectDescriptor>::Error,
        >,
    > {
        Box::pin(async move {
            // Replace with a real persistence boundary. The Driver, not the
            // Component, owns the side effect.
            let _ = descriptor;
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

    /// Exact source capability used by live and controlled stream adapters.
    increments: SourceCapability<StreamDescriptor<u64>>,
}

/// Declares the logical application graph shared by every execution profile.
fn program(increments: StreamDescriptor<u64>) -> (AppProgram, AppRefs) {
    let (mut program, (increment_source, persistence)) =
        Program::builder::<(Source<StreamDescriptor<u64>>, Effect<PersistCount>)>();

    // A named Port is immutable dependency wiring. Naming allows another Quota
    // Port to coexist without a global “one provider per type” rule.
    let quota_port = program.port(PortId::new(QUOTA));
    let quota = program
        .component::<Quota>(ComponentId::new("quota"), QuotaConfig { initial: 100 })
        .finish()
        .expect("component registration");

    // Assembly chooses the concrete provider. `QuotaMessage` declares its
    // canonical Port-message conversion through `From`; Counter never needs to
    // know that private Message vocabulary.
    program.connect(&quota_port, &quota.connector());
    let counter = program
        .component::<Counter>(
            ComponentId::new("counter"),
            CounterConfig {
                increments: increments.clone(),
                quota: quota_port,
            },
        )
        .with_capabilities((increment_source.clone(), persistence))
        .finish()
        .expect("component registration");

    (
        program.build().expect("the example graph is valid"),
        AppRefs {
            counter,
            quota,
            increments: increment_source,
        },
    )
}

/// Shows production-shaped execution with Tokio-backed world boundaries.
///
/// This function is compile-checked documentation in the candidate API. It uses
/// the same `program` as controlled execution, but binds real receivers and an
/// async effect Driver before spawning the live runtime.
async fn live_shape() -> Result<(), RuntimeError> {
    let increments = StreamDescriptor::named(INCREMENT_INPUT);
    let (input, receiver) = tokio::sync::mpsc::channel(32);
    let (program, refs) = program(increments.clone());

    // Runtime assembly owns the operational channel receiver and effect
    // executor. The logical capabilities and descriptors inside Components
    // stay inert.
    let runtime = Runtime::builder(program)
        .with_drivers((samara::MpscReceiver::new(receiver), LivePersistence))
        .build()?;

    let counter = runtime.handle(&refs.counter)?;
    let runtime = runtime.spawn();

    // Both a driven source and an explicit low-level send become
    // CounterMessage values. Neither path may mutate CounterModel directly.
    let _ = input.send(3).await;
    counter.send(CounterMessage::Increment(2)).await?;

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
    let increments = StreamDescriptor::named(INCREMENT_INPUT);
    let (program, refs) = program(increments.clone());

    let mut runtime = ControlledRuntime::builder(program).control_all().build()?;

    // This single controlled input drives the full causal chain:
    // Increment -> quota request -> Quota transition -> reply -> Counter transition.
    runtime.emit_stream(&refs.increments, 3)?;
    runtime.run_until_idle()?;

    // The world-facing persistence intent is now pending, rather than having
    // executed invisibly. The harness chooses its terminal outcome and timing.
    let pending = runtime.next_effect::<PersistCount>()?;
    assert_eq!(pending.intent, PersistCount { value: 3 });
    runtime.complete(pending, EffectOutcome::Succeeded(()))?;
    runtime.run_until_idle()?;

    // Advancing logical time deterministically delivers the scheduled Tick.
    runtime.advance(Duration::from_secs(1))?;

    // State inspection proves the outcome across both independently owned
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

/// Keeps the binary intentionally inert; the tests exercise both profiles.
fn main() {
    println!("Samara live and controlled reference; see this source and its tests");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Activates the controlled counterpart of the broad API-pressure example
    /// without executing a live Driver.
    #[test]
    fn controlled_reference_program_runs_end_to_end() -> Result<(), RuntimeError> {
        controlled_shape()
    }

    /// Runs the unchanged Components and Program through real Tokio scheduling,
    /// Ports, a first-party channel bridge, a terminal Effect Driver, and
    /// structured cancellation.
    #[tokio::test]
    async fn live_reference_program_runs_end_to_end() -> Result<(), RuntimeError> {
        live_shape().await
    }

    /// Proves pure transition intent without constructing either runtime.
    ///
    /// Runtime-level behavior belongs in controlled tests; this fast unit test
    /// checks the Component contract directly: a request does not prematurely
    /// mutate state, an injected grant produces state plus a typed effect, and
    /// the desired subscription remains inspectable data.
    #[test]
    fn transition_and_subscription_shape_are_directly_testable() {
        let increments = StreamDescriptor::named(INCREMENT_INPUT);
        let (mut program, (increment_source, persistence)) =
            Program::builder::<(Source<StreamDescriptor<u64>>, Effect<PersistCount>)>();

        let quota = program.port(PortId::new(QUOTA));
        let (component, component_capabilities) = (
            CounterConfig {
                increments: increments.clone(),
                quota,
            },
            (increment_source, persistence),
        );
        let mut model = Counter::init(&component, &component_capabilities).model;

        // Dispatching a request declares intent only. The later reply is the sole
        // message that can authorize this change.
        let command = Counter::update(
            &component,
            &component_capabilities,
            &mut model,
            CounterMessage::Increment(3),
        );
        assert_eq!(model.count, 0);
        let request = command
            .into_request_to::<QuotaProtocolMessage>()
            .unwrap_or_else(|_| panic!("an increment should request a reservation"));
        assert!(matches!(
            request.message,
            QuotaProtocolMessage::Reserve { amount: 3, .. }
        ));

        // Running the request's own continuation on a chosen reply exercises
        // the resulting transition independently of transport correlation.
        let granted = request.reply(RequestOutcome::Replied(Reservation::Granted { amount: 3 }));
        let command = Counter::update(&component, &component_capabilities, &mut model, granted);
        assert_eq!(model.count, 3);
        let effect = command
            .into_effect::<PersistCount>()
            .unwrap_or_else(|_| panic!("the granted reservation should request persistence"));
        assert_eq!(effect.descriptor(), &PersistCount { value: 3 });

        // Phase 4 can exercise the exact stored one-shot mapper without
        // pretending a Driver or controlled terminal behavior ran.
        let persisted = effect.map_outcome(EffectOutcome::Succeeded(()));
        let command = Counter::update(&component, &component_capabilities, &mut model, persisted);
        assert!(command.is_none());
        assert_eq!(model.last_save, Some(EffectOutcome::Succeeded(())));

        // Subscription configuration is declared and testable without running
        // the receiver or depending on a particular task/mailbox topology.
        let subscriptions = Counter::subscriptions(&component, &component_capabilities, &model);
        assert_eq!(
            subscriptions
                .find::<StreamDescriptor<u64>>(&SubscriptionId::new(INCREMENT_SUBSCRIPTION)),
            Some(&increments)
        );
    }
}
