//! A deliberately complete consumer sketch for a framed TCP telemetry feed.
//!
//! The example makes Samara's architectural boundaries visible before those
//! boundaries are backed by a real runtime implementation:
//!
//! - Tokio-facing code produces raw TCP byte chunks.
//! - A pure, stateful decoder turns arbitrarily split chunks into frames.
//! - `Telemetry` owns application state and declares the feed it currently
//!   wants as a reconciled subscription.
//! - Storage is a one-shot effect; health reporting crosses a named protocol
//!   Port through both notifications and correlated requests.
//! - Live and controlled runtimes consume the same Component definitions and
//!   differ only in how runtime decisions and interactions with the world are
//!   supplied.
//!
//! This is an API-pressure example, not a production TCP implementation. Its
//! purpose is to show what application authors should have to express—and what
//! transport, correlation, reconciliation, and shutdown machinery Samara
//! should own for them.

#![allow(dead_code)]

use std::{future::pending, sync::Arc};

use samara_api_sketch::prelude::*;

/// Stable identity for the telemetry feed's reconciled subscription.
const SOCKET: &str = "telemetry/socket";

/// Assembly-time identity of the health dependency used by `Telemetry`.
const HEALTH: &str = "health/primary";

/// An application-level socket destination, kept free of live socket handles.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Endpoint(Arc<str>);

impl Endpoint {
    /// Creates a cheaply cloneable endpoint value suitable for Models and specs.
    fn new(value: impl Into<Arc<str>>) -> Self {
        Self(value.into())
    }
}

/// A declarative request for raw bytes from one generation of a TCP feed.
///
/// This world-facing source descriptor intentionally says nothing about
/// framing. `generation` lets otherwise identical reconnect attempts produce a
/// changed subscription spec that the runtime can reconcile.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TcpBytes {
    /// Where the transport adapter should connect.
    endpoint: Endpoint,

    /// Application-owned identity for this connection attempt.
    generation: u64,
}

impl Source for TcpBytes {
    type Item = Vec<u8>;
    type Error = TcpFailure;
}

/// Failures owned by the TCP transport mechanism rather than the frame decoder.
#[derive(Clone, Debug, PartialEq, Eq)]
enum TcpFailure {
    /// Establishing the connection failed.
    Connect(String),

    /// Reading an established connection failed.
    Read(String),
}

/// One complete application frame after transport chunks have been decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Frame(Vec<u8>);

/// Pure configuration. The incomplete-buffer state belongs to `DecoderState`,
/// which the runtime-owned framing adapter creates for each subscription.
#[derive(Clone, Debug, PartialEq, Eq)]
struct U16LengthDelimited {
    /// Policy supplied by the application to bound accepted frame sizes.
    max_frame_len: usize,
}

/// Per-subscription framing state owned by the framing adapter.
///
/// It is deliberately absent from `TelemetryModel`: buffering partial wire
/// data is protocol mechanism, not durable application truth.
#[derive(Debug, Default)]
struct DecoderState {
    /// Bytes retained until one or more complete frames can be emitted.
    buffered: Vec<u8>,
}

/// Framing failures, kept distinct from transport failures by `FramedFailure`.
#[derive(Clone, Debug, PartialEq, Eq)]
enum DecodeFailure {
    /// A length prefix exceeded the configured resource bound.
    FrameTooLarge { length: usize, maximum: usize },
}

impl Decoder for U16LengthDelimited {
    type Chunk = Vec<u8>;
    type Frame = Frame;
    type Error = DecodeFailure;
    type State = DecoderState;

    fn start(&self) -> Self::State {
        DecoderState::default()
    }

    /// Consumes an arbitrary transport chunk and emits every complete frame it
    /// makes available, retaining only an incomplete suffix in `state`.
    fn push(
        &self,
        state: &mut Self::State,
        chunk: Self::Chunk,
    ) -> Result<Vec<Self::Frame>, Self::Error> {
        state.buffered.extend(chunk);
        let mut frames = Vec::new();

        loop {
            // A u16 length prefix may itself be split across TCP reads.
            if state.buffered.len() < 2 {
                break;
            }

            let length = u16::from_be_bytes([state.buffered[0], state.buffered[1]]) as usize;
            if length > self.max_frame_len {
                return Err(DecodeFailure::FrameTooLarge {
                    length,
                    maximum: self.max_frame_len,
                });
            }

            // Keep the prefix and payload until the frame is complete.
            if state.buffered.len() < length + 2 {
                break;
            }

            // One chunk may contain several frames, so continue after draining
            // exactly the decoded prefix and payload.
            let bytes = state.buffered[2..length + 2].to_vec();
            state.buffered.drain(..length + 2);
            frames.push(Frame(bytes));
        }

        Ok(frames)
    }
}

/// The application-visible source after composing transport and framing.
type TelemetrySource = Framed<TcpBytes, U16LengthDelimited>;

/// Preserves whether a feed ended because transport or decoding failed.
type TelemetryFailure = FramedFailure<TcpFailure, DecodeFailure>;

/// Live Tokio owns only transport I/O. Framing remains the same pure adapter in
/// live and controlled profiles.
struct TokioTcpBytes;

impl SourceAdapter<TcpBytes> for TokioTcpBytes {
    /// Bridges live Tokio I/O into Samara's source sink; it never sees or
    /// mutates a Component Model.
    fn run(&self, source: TcpBytes, sink: SourceSink<TcpBytes>) -> BoxFuture<()> {
        Box::pin(async move {
            loop {
                match read_chunk(&source.endpoint).await {
                    Ok(Some(bytes)) => {
                        if sink.emit(bytes).await.is_err() {
                            return;
                        }
                    }
                    Ok(None) => {
                        let _ = sink.end().await;
                        return;
                    }
                    Err(error) => {
                        let _ = sink.fail(error).await;
                        return;
                    }
                }
            }
        })
    }
}

/// Stands in for the eventual first-party Tokio TCP adapter's socket read.
async fn read_chunk(_endpoint: &Endpoint) -> Result<Option<Vec<u8>>, TcpFailure> {
    // Placeholder for Tokio socket reads. This never runs in the compile-only
    // sketch, but makes the adapter boundary and ownership shape concrete.
    pending().await
}

/// All mutable state owned by the health Component.
#[derive(Debug, Default, PartialEq, Eq)]
struct HealthModel {
    /// Human-readable summary most recently derived from protocol input.
    status: String,

    /// Count of frame notifications processed by this Component.
    frames_seen: u64,
}

protocol! {
    /// Public interaction contract offered by the health provider.
    ///
    /// Requesters depend on this protocol through a `Port<HealthProtocol>`; they
    /// do not depend on `Health`, `HealthMsg`, or its Model representation. The
    /// generated marker remains a normal, nameable Port parameter.
    type HealthProtocol => enum HealthInbound {
        // One-way health observations with no correlated terminal result.
        Connecting(Endpoint),
        FrameSeen,
        Disconnected,
        Failed(String),

        // A correlated snapshot request carrying a runtime-created reply route.
        Read -> HealthSnapshot,
    }
}

/// Typed response paired with `Read` at the protocol declaration site.
#[derive(Clone, Debug, PartialEq, Eq)]
struct HealthSnapshot {
    /// Provider status at the transition that handled the request.
    status: String,

    /// Number of frame notices handled before that transition.
    frames_seen: u64,
}

/// Private message vocabulary of the health Component.
///
/// Port users never need to name this type; assembly supplies the conversion
/// from public protocol input to this Component-specific message.
#[derive(Debug)]
enum HealthMsg {
    /// An operation delivered through a bound `HealthProtocol` Port.
    Protocol(HealthInbound),
}

/// State-owning provider for `HealthProtocol`.
struct Health;

impl Component for Health {
    type Model = HealthModel;
    type Msg = HealthMsg;

    fn init(&self) -> Init<Self::Model, Self::Msg> {
        Init::new(HealthModel::default())
    }

    fn update(&self, model: &mut Self::Model, msg: Self::Msg) -> Cmd<Self::Msg> {
        let HealthMsg::Protocol(incoming) = msg;

        match incoming {
            // Notifications update only this Component's Model. No sender
            // observes completion and no transport handle leaks in. Each
            // operation is a direct inbound variant rather than a nested enum.
            HealthInbound::Connecting(endpoint) => {
                model.status = format!("connecting to {}", endpoint.0);
                Cmd::none()
            }
            HealthInbound::FrameSeen => {
                model.status = "receiving".to_owned();
                model.frames_seen += 1;
                Cmd::none()
            }
            HealthInbound::Disconnected => {
                model.status = "disconnected".to_owned();
                Cmd::none()
            }
            HealthInbound::Failed(error) => {
                model.status = format!("failed: {error}");
                Cmd::none()
            }
            HealthInbound::Read(incoming) => {
                // `ReplyTo` identifies this dynamic invocation. The provider
                // returns a typed reply intent without inspecting correlation
                // IDs or sending directly from async code.
                let _ = incoming.request;
                Cmd::reply(
                    incoming.reply_to,
                    HealthSnapshot {
                        status: model.status.clone(),
                        frames_seen: model.frames_seen,
                    },
                )
            }
        }
    }
}

/// Application-owned description of the feed `Telemetry` wants to exist.
#[derive(Clone, Debug, PartialEq, Eq)]
struct FeedConfig {
    /// Desired destination.
    endpoint: Endpoint,

    /// Identity of the desired connection attempt.
    generation: u64,

    /// Framing policy applied identically in live and controlled execution.
    max_frame_len: usize,
}

/// Single source of mutable application truth for the telemetry Component.
#[derive(Debug, Default, PartialEq, Eq)]
struct TelemetryModel {
    /// `Some` declares a desired feed; `None` declares no subscription.
    desired: Option<FeedConfig>,

    /// Frames accepted from the reconciled source.
    received: u64,

    /// Storage effects that later reported success.
    stored: u64,

    /// Most recent source or storage failure rendered for this sketch.
    last_failure: Option<String>,

    /// Request outcomes paired with requester-owned domain correlation.
    health_checks: Vec<(ProbeId, RequestOutcome<HealthSnapshot>)>,
}

/// Every message capable of transitioning `TelemetryModel`.
#[derive(Debug)]
enum TelemetryMsg {
    /// Declare or replace the desired feed configuration.
    Connect(FeedConfig),

    /// Remove the desired feed.
    Disconnect,

    /// Lifecycle or item event emitted by the framed source.
    Socket(SourceEvent<Frame, TelemetryFailure>),

    /// Completion event for a previously emitted storage effect.
    Stored(EffectEvent<FrameId, StoreFailure>),

    /// Issue two independent snapshot requests from one transition.
    CheckHealth([ProbeId; 2]),

    /// A health request outcome tagged with the requester's domain identity.
    HealthChecked {
        /// Meaningful application correlation captured with the request.
        probe: ProbeId,
        /// Runtime lifecycle outcome, including reply or timeout.
        event: RequestOutcome<HealthSnapshot>,
    },
}

/// Requester-owned identity explaining which logical probe a reply belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProbeId(u64);

/// Identifier returned after a frame is durably stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FrameId(u64);

/// Typed, finite intent to persist one decoded frame.
#[derive(Clone, Debug, PartialEq, Eq)]
struct StoreFrame {
    /// Immutable payload handed to the storage effect adapter.
    frame: Frame,
}

impl Effect for StoreFrame {
    type Output = FrameId;
    type Error = StoreFailure;
}

/// Application-visible failure reported by the storage adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
struct StoreFailure(String);

/// Telemetry Component definition and its immutable, assembly-supplied wiring.
///
/// The Port belongs here because it is a stable dependency of this Component
/// instance. Mutable feed state belongs in `TelemetryModel`; live connection
/// and decoder state belong in runtime adapters.
struct Telemetry {
    /// Named protocol dependency, independent of the provider's message enum.
    health: Port<HealthProtocol>,
}

/// Builds the pure continuation for one asynchronous health request.
///
/// Ordinary function calls get dynamic correlation from the call stack. A
/// dispatched request has no shared stack with its eventual reply, so the mapper
/// explicitly captures the domain context while Samara owns transport-level
/// correlation.
fn health_checked(
    probe: ProbeId,
) -> impl FnOnce(RequestOutcome<HealthSnapshot>) -> TelemetryMsg + Send + 'static {
    move |event| TelemetryMsg::HealthChecked { probe, event }
}

impl Component for Telemetry {
    type Model = TelemetryModel;
    type Msg = TelemetryMsg;

    fn init(&self) -> Init<Self::Model, Self::Msg> {
        Init::new(TelemetryModel::default())
    }

    fn update(&self, model: &mut Self::Model, msg: Self::Msg) -> Cmd<Self::Msg> {
        match msg {
            TelemetryMsg::Connect(config) => {
                // Changing the Model changes the desired subscription returned
                // below; the runtime performs the actual start or replacement.
                model.desired = Some(config.clone());
                model.last_failure = None;
                Cmd::notify(self.health.clone(), Connecting(config.endpoint))
            }
            TelemetryMsg::Disconnect => {
                // Removing desire is enough. Reconciliation and cancellation
                // are runtime work, not reducer-side effects.
                model.desired = None;
                Cmd::notify(self.health.clone(), Disconnected)
            }
            TelemetryMsg::Socket(SourceEvent::Item(frame)) => {
                model.received += 1;
                // One transition can declare independent intents without
                // performing either operation inline.
                Cmd::batch([
                    Cmd::notify(self.health.clone(), FrameSeen),
                    Cmd::effect(StoreFrame { frame }, TelemetryMsg::Stored),
                ])
            }
            TelemetryMsg::Socket(SourceEvent::Failed(error)) => {
                // Failure is terminal. Reconnect policy must arrive as another
                // explicit message with a new desired configuration.
                model.desired = None;
                let error = format!("{error:?}");
                model.last_failure = Some(error.clone());
                Cmd::notify(self.health.clone(), Failed(error))
            }
            TelemetryMsg::Socket(SourceEvent::Ended) => {
                model.desired = None;
                Cmd::notify(self.health.clone(), Disconnected)
            }
            TelemetryMsg::Stored(EffectEvent::Succeeded(_frame_id)) => {
                // Async completion re-enters the Component only as a Msg.
                model.stored += 1;
                Cmd::none()
            }
            TelemetryMsg::Stored(EffectEvent::Failed(error)) => {
                model.last_failure = Some(error.0);
                Cmd::none()
            }
            TelemetryMsg::Stored(EffectEvent::Cancelled(reason)) => {
                model.last_failure = Some(format!("store cancelled: {reason:?}"));
                Cmd::none()
            }
            TelemetryMsg::CheckHealth(probes) => {
                // Both requests become outstanding from one transition. Completion
                // order is intentionally not assumed; each pure mapper captures
                // the domain key needed to interpret its eventual reply.
                Cmd::batch(
                    probes.map(|probe| {
                        Cmd::request(self.health.clone(), Read, health_checked(probe))
                    }),
                )
            }
            TelemetryMsg::HealthChecked { probe, event } => {
                model.health_checks.push((probe, event));
                Cmd::none()
            }
        }
    }

    fn subscriptions(&self, model: &Self::Model) -> Subscriptions<Self::Msg> {
        // Subscriptions are a pure projection of current Model state, analogous
        // to declarative resource desire rather than imperative task spawning.
        let Some(config) = &model.desired else {
            return Subscriptions::none();
        };

        let source = Framed::new(
            TcpBytes {
                endpoint: config.endpoint.clone(),
                generation: config.generation,
            },
            U16LengthDelimited {
                max_frame_len: config.max_frame_len,
            },
        );

        // Reconciliation compares both stable identity and typed spec: absence
        // stops it, equal identity/spec preserves it, and changed spec replaces
        // it while keeping the logical subscription identity.
        Subscriptions::one(Subscription::source(
            SubscriptionId::new(SOCKET),
            source,
            TelemetryMsg::Socket,
        ))
    }
}

/// Live adapter for the `StoreFrame` effect boundary.
///
/// Production code would perform Tokio-backed storage here. App retry or
/// classification policy would still be modeled by messages and Components.
struct LiveFrameStore;

impl EffectAdapter<StoreFrame> for LiveFrameStore {
    fn execute(
        &self,
        effect: StoreFrame,
    ) -> BoxFuture<Result<<StoreFrame as Effect>::Output, <StoreFrame as Effect>::Error>> {
        Box::pin(async move {
            let _ = effect;
            Ok(FrameId(1))
        })
    }
}

/// Handles retained by assembly so hosts and tests can address Components and
/// inspect the named dependency without exposing provider internals to consumers.
struct AppRefs {
    /// Direct low-level reference to the health Component.
    health: ComponentRef<Health>,

    /// Named protocol boundary bound to the health provider.
    health_port: Port<HealthProtocol>,

    /// Reference used by the host to send telemetry messages.
    telemetry: ComponentRef<Telemetry>,
}

/// Declares the Component graph and resolves protocol dependency injection.
///
/// Port declaration, provider binding, and consumer construction happen in one
/// explicit composition root. The runtime receives a closed `Program` rather
/// than asking Components to discover dependencies dynamically.
fn program() -> (Program, AppRefs) {
    let mut program = Program::builder();

    // A named Port permits multiple independently bound instances of the same
    // protocol; the protocol's Rust type alone is not a global service key.
    let health_port = program.port(PortId::new(HEALTH));
    let health = program.component(ComponentId::new("health"), Health);

    // Assembly owns the only knowledge that public `HealthInbound` values enter
    // this particular provider as `HealthMsg::Protocol`.
    program.bind_port(&health_port, &health, HealthMsg::Protocol);
    let telemetry = program.component(
        ComponentId::new("telemetry"),
        Telemetry {
            health: health_port.clone(),
        },
    );

    (
        program.build(),
        AppRefs {
            health,
            health_port,
            telemetry,
        },
    )
}

/// Convenience constructor for the examples' desired feed configuration.
fn config(endpoint: &str, generation: u64) -> FeedConfig {
    FeedConfig {
        endpoint: Endpoint::new(endpoint),
        generation,
        max_frame_len: 4096,
    }
}

/// Shows the live host shape: bind real adapters, spawn, interact, and perform
/// structured shutdown through the runtime owner.
async fn live_shape() -> Result<(), RuntimeError> {
    let (program, refs) = program();
    let runtime = LiveRuntime::builder(program)
        .bind_source::<TcpBytes, _>(TokioTcpBytes)
        .bind_effect::<StoreFrame, _>(LiveFrameStore)
        .build()?;

    let telemetry = runtime.handle(&refs.telemetry)?;
    let runtime = runtime.spawn();

    // Host interaction still enters through a Msg; obtaining a handle does not
    // grant direct access to the Component's Model.
    telemetry
        .send(TelemetryMsg::Connect(config("127.0.0.1:7000", 1)))
        .await?;

    // Runtime-owned work is joined or cancelled as one structured lifetime.
    // The returned report makes incomplete cleanup observable to the host.
    let report = runtime.shutdown(Shutdown::Cancel).await?;
    assert!(report.is_clean());
    Ok(())
}

/// Shows the controlled host shape using the same Program and Component code.
///
/// The harness supplies source events and effect outcomes explicitly, allowing
/// deterministic traces and faster-than-real-time execution without changing
/// application semantics.
fn controlled_shape() -> Result<(), RuntimeError> {
    let (program, refs) = program();
    let mut runtime = ControlledRuntime::builder(program)
        .control_source::<TcpBytes>()
        .control_effect::<StoreFrame>()
        .build()?;

    runtime.send(
        &refs.telemetry,
        TelemetryMsg::Connect(config("simulated:7000", 1)),
    )?;
    runtime.run_until_idle()?;

    // Controlled execution exposes declared runtime state for assertions; it
    // does not require poking at an adapter's private task or socket.
    let socket = SubscriptionId::new(SOCKET);
    let active: TelemetrySource = runtime.subscription_spec(&refs.telemetry, &socket)?;
    assert_eq!(active.source.endpoint, Endpoint::new("simulated:7000"));

    // Inject raw chunks at the TcpBytes layer. The first chunk is partial; the
    // second completes it and contains another whole frame. The same decoder
    // therefore runs in controlled and live execution.
    runtime.emit_source::<Telemetry, TcpBytes>(
        &refs.telemetry,
        &socket,
        SourceEvent::Item(vec![0, 3, b'a']),
    )?;
    runtime.emit_source::<Telemetry, TcpBytes>(
        &refs.telemetry,
        &socket,
        SourceEvent::Item(vec![b'b', b'c', 0, 1, b'x']),
    )?;
    runtime.run_until_idle()?;

    // Effects remain pending, inspectable intents until the controlled world
    // supplies their outcomes.
    let first = runtime.next_effect::<StoreFrame>()?;
    assert_eq!(first.intent.frame, Frame(b"abc".to_vec()));
    let second = runtime.next_effect::<StoreFrame>()?;
    assert_eq!(second.intent.frame, Frame(b"x".to_vec()));
    runtime.complete(first, EffectEvent::Succeeded(FrameId(1)))?;
    runtime.complete(second, EffectEvent::Succeeded(FrameId(2)))?;
    runtime.run_until_idle()?;
    assert_eq!(runtime.state(&refs.telemetry)?.stored, 2);

    // Same subscription identity, changed resource configuration: replace it.
    runtime.send(
        &refs.telemetry,
        TelemetryMsg::Connect(config("simulated:8000", 2)),
    )?;
    runtime.run_until_idle()?;
    let replaced: TelemetrySource = runtime.subscription_spec(&refs.telemetry, &socket)?;
    assert_ne!(active, replaced);

    runtime.emit_source::<Telemetry, TcpBytes>(
        &refs.telemetry,
        &socket,
        SourceEvent::Failed(TcpFailure::Read("connection reset".to_owned())),
    )?;
    runtime.run_until_idle()?;
    assert!(runtime.state(&refs.telemetry)?.desired.is_none());

    // A conformance harness can compare this salient semantic trace across
    // runtime implementations without depending on their scheduler topology.
    let _semantic_trace = runtime.trace();

    // The source failure removed subscription desire, and both finite storage
    // effects were completed above. Scope cancellation is still the explicit
    // proof that no runtime-owned controlled work escaped the scenario.
    assert_eq!(runtime.state(&refs.telemetry)?.stored, 2);
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

    /// Proves the pure decoder handles both transport fragmentation and several
    /// application frames coalesced into one read.
    #[test]
    fn decoder_handles_split_and_coalesced_frames() {
        let decoder = U16LengthDelimited { max_frame_len: 8 };
        let mut state = decoder.start();

        assert_eq!(
            decoder.push(&mut state, vec![0, 3, b'a']).unwrap(),
            Vec::<Frame>::new()
        );
        assert_eq!(
            decoder
                .push(&mut state, vec![b'b', b'c', 0, 1, b'x'])
                .unwrap(),
            vec![Frame(b"abc".to_vec()), Frame(b"x".to_vec())]
        );
    }

    /// Proves reconciliation can observe a spec replacement even though the
    /// logical subscription retains the same stable identity.
    #[test]
    fn same_subscription_identity_exposes_changed_configuration() {
        let mut builder = Program::builder();
        let health = builder.port(PortId::new(HEALTH));
        let component = Telemetry { health };
        let mut model = component.init().model;
        let id = SubscriptionId::new(SOCKET);

        component.update(&mut model, TelemetryMsg::Connect(config("one:7000", 1)));
        let first = component.subscriptions(&model);
        let first = first.find::<TelemetrySource>(&id).unwrap();

        component.update(&mut model, TelemetryMsg::Connect(config("two:7000", 2)));
        let second = component.subscriptions(&model);
        let second = second.find::<TelemetrySource>(&id).unwrap();

        assert_ne!(first, second);
        assert_eq!(first.source.endpoint, Endpoint::new("one:7000"));
        assert_eq!(second.source.endpoint, Endpoint::new("two:7000"));
    }

    /// Proves named Ports, rather than protocol types, identify dependencies at
    /// assembly time, so one protocol may have multiple bindings.
    #[test]
    fn named_ports_bind_independently_even_for_the_same_protocol() {
        let mut builder = Program::builder();
        let primary = builder.port::<HealthProtocol>(PortId::new("health/primary"));
        let fallback = builder.port::<HealthProtocol>(PortId::new("health/fallback"));
        let provider = builder.component(ComponentId::new("health"), Health);

        builder.bind_port(&primary, &provider, HealthMsg::Protocol);
        builder.bind_port(&fallback, &provider, HealthMsg::Protocol);

        assert_ne!(primary, fallback);
        assert_eq!(primary.id(), &PortId::new("health/primary"));
        assert_eq!(fallback.id(), &PortId::new("health/fallback"));
    }

    /// Proves consumers emit public protocol notifications without importing or
    /// constructing the provider's private `HealthMsg` vocabulary.
    #[test]
    fn telemetry_emits_protocol_notifications_without_provider_messages() {
        let mut builder = Program::builder();
        let health = builder.port(PortId::new(HEALTH));
        let component = Telemetry { health };
        let mut model = component.init().model;

        let cmd = component.update(&mut model, TelemetryMsg::Connect(config("one:7000", 1)));
        let notifications = cmd.notification_intents::<Connecting>();

        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].0, &PortId::new(HEALTH));
        assert_eq!(notifications[0].1.0, Endpoint::new("one:7000"));
    }

    /// Proves concurrent requests use one result Msg variant while preserving
    /// requester-owned identity independently of completion order.
    #[test]
    fn two_requests_share_one_result_variant_and_keep_domain_correlation() {
        fn assert_reply_type<R>(_request: &R)
        where
            R: Request<HealthProtocol, Reply = HealthSnapshot>,
        {
        }

        let mut builder = Program::builder();
        let health = builder.port(PortId::new(HEALTH));
        let component = Telemetry { health };
        let mut model = component.init().model;

        let cmd = component.update(
            &mut model,
            TelemetryMsg::CheckHealth([ProbeId(17), ProbeId(29)]),
        );
        let requests = cmd.request_intents::<Read>();

        assert_eq!(requests.len(), 2);
        assert!(
            requests
                .iter()
                .all(|(port, _)| *port == &PortId::new(HEALTH))
        );
        assert_reply_type(requests[0].1);
        assert_reply_type(requests[1].1);

        let snapshot = HealthSnapshot {
            status: "receiving".to_owned(),
            frames_seen: 8,
        };
        let first = health_checked(ProbeId(17))(RequestOutcome::Replied(snapshot.clone()));
        let second = health_checked(ProbeId(29))(RequestOutcome::TimedOut);

        component.update(&mut model, second);
        component.update(&mut model, first);
        assert_eq!(
            model.health_checks,
            vec![
                (ProbeId(29), RequestOutcome::TimedOut),
                (ProbeId(17), RequestOutcome::Replied(snapshot)),
            ]
        );
    }
}
