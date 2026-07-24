//! A deliberately complete reference Component for a framed TCP telemetry feed.
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

use samara::prelude::*;

/// Stable identity for the telemetry feed's reconciled subscription.
const SOCKET: &str = "telemetry/socket";

/// Assembly-time identity of the health dependency used by `Telemetry`.
const HEALTH: &str = "health/primary";

/// An application-level socket destination, kept free of live socket handles.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Endpoint(Arc<str>);

impl Endpoint {
    /// Creates a cheaply cloneable endpoint value suitable for Models and descriptors.
    fn new(value: impl Into<Arc<str>>) -> Self {
        Self(value.into())
    }
}

/// A declarative request for raw bytes from one generation of a TCP feed.
///
/// This world-facing source descriptor intentionally says nothing about
/// framing. `generation` lets otherwise identical reconnect attempts produce a
/// changed SourceDescriptor that the runtime can reconcile.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TcpBytes {
    /// Where the terminal transport Driver should connect.
    endpoint: Endpoint,

    /// Application-owned identity for this connection attempt.
    generation: u64,
}

impl SourceDescriptor for TcpBytes {
    type Item = Vec<u8>;
    type Error = TcpError;
}

/// Error data owned by the TCP transport mechanism rather than the frame decoder.
#[derive(Clone, Debug, PartialEq, Eq)]
enum TcpError {
    /// Establishing the connection failed.
    Connect(String),

    /// Reading an established connection failed.
    Read(String),
}

/// One complete application frame after transport chunks have been decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Frame(Vec<u8>);

/// Pure configuration. The incomplete-buffer state belongs to `DecoderState`,
/// which the runtime-owned framing Layer creates for each subscription.
#[derive(Clone, Debug, PartialEq, Eq)]
struct U16LengthDelimited {
    /// Policy supplied by the application to bound accepted frame sizes.
    max_frame_len: usize,
}

/// Per-subscription framing state owned by the framing Layer.
///
/// It is deliberately absent from `TelemetryModel`: buffering partial wire
/// data is protocol mechanism, not durable application truth.
#[derive(Debug, Default)]
struct DecoderState {
    /// Bytes retained until one or more complete frames can be emitted.
    buffered: Vec<u8>,
}

/// Framing error data, kept distinct from transport errors by `FramedError`.
#[derive(Clone, Debug, PartialEq, Eq)]
enum DecodeError {
    /// A length prefix exceeded the configured resource bound.
    FrameTooLarge { length: usize, maximum: usize },

    /// The peer ended while a length prefix or frame payload was incomplete.
    UnexpectedEof,
}

impl Decoder for U16LengthDelimited {
    type Chunk = Vec<u8>;
    type Frame = Frame;
    type Error = DecodeError;
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
                return Err(DecodeError::FrameTooLarge {
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

    fn finish(&self, state: &mut Self::State) -> Result<Vec<Self::Frame>, Self::Error> {
        if state.buffered.is_empty() {
            Ok(Vec::new())
        } else {
            Err(DecodeError::UnexpectedEof)
        }
    }
}

/// The application-visible feed descriptor after composing transport and framing.
type TelemetryFeed = Framed<TcpBytes, U16LengthDelimited>;

/// Error data preserving whether transport or decoding caused a feed failure.
type TelemetryError = FramedError<TcpError, DecodeError>;

/// Live Tokio owns only transport I/O. Framing remains the same pure Layer in
/// live and controlled profiles.
struct TokioTcpBytes;

impl SourceDriver<TcpBytes> for TokioTcpBytes {
    /// Bridges live Tokio I/O into Samara's source sink; it never sees or
    /// mutates a Component Model.
    fn run(&self, descriptor: TcpBytes, sink: SourceSink<TcpBytes>) -> BoxFuture<()> {
        Box::pin(async move {
            loop {
                match read_chunk(&descriptor.endpoint).await {
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

/// Stands in for the eventual first-party Tokio TCP Driver's socket read.
async fn read_chunk(_endpoint: &Endpoint) -> Result<Option<Vec<u8>>, TcpError> {
    // Placeholder for Tokio socket reads. This never runs in the compile-only
    // reference, but makes the Driver boundary and ownership shape concrete.
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
    /// do not depend on `Health`, `HealthMessage`, or its Model representation.
    /// The generated marker remains a normal, nameable Port parameter.
    type HealthProtocol => enum HealthProtocolMessage {
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
/// from a public protocol message to this Component-specific message.
#[derive(Debug)]
enum HealthMessage {
    /// An operation delivered through a bound `HealthProtocol` Port.
    Protocol(HealthProtocolMessage),
}

impl From<HealthProtocolMessage> for HealthMessage {
    fn from(value: HealthProtocolMessage) -> Self {
        Self::Protocol(value)
    }
}

/// State-owning provider for `HealthProtocol`.
struct Health;

impl Component for Health {
    type Model = HealthModel;
    type Message = HealthMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(HealthModel::default())
    }

    fn update(
        &self,
        model: &mut Self::Model,
        HealthMessage::Protocol(message): Self::Message,
    ) -> Command<Self::Message> {
        match message {
            // Notifications update only this Component's Model. No sender
            // observes completion and no transport handle leaks in. Each
            // operation is a direct protocol-message variant rather than a
            // nested enum.
            HealthProtocolMessage::Connecting(endpoint) => {
                model.status = format!("connecting to {}", endpoint.0);
                Command::none()
            }
            HealthProtocolMessage::FrameSeen => {
                model.status = "receiving".to_owned();
                model.frames_seen += 1;
                Command::none()
            }
            HealthProtocolMessage::Disconnected => {
                model.status = "disconnected".to_owned();
                Command::none()
            }
            HealthProtocolMessage::Failed(error) => {
                model.status = format!("failed: {error}");
                Command::none()
            }
            HealthProtocolMessage::Read(invocation) => {
                // `ReplyTo` identifies this dynamic invocation. The provider
                // returns a typed reply intent without inspecting correlation
                // IDs or sending directly from async code.
                let _ = invocation.request;
                Command::reply(
                    invocation.reply_to,
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

    /// Most recent source or storage failure rendered for this reference.
    last_failure: Option<String>,

    /// Request outcomes paired with requester-owned domain correlation.
    health_checks: Vec<(ProbeId, RequestOutcome<HealthSnapshot>)>,
}

/// Every message capable of transitioning `TelemetryModel`.
#[derive(Debug)]
enum TelemetryMessage {
    /// Declare or replace the desired feed configuration.
    Connect(FeedConfig),

    /// Remove the desired feed.
    Disconnect,

    /// Lifecycle or item event emitted by the framed source.
    Socket(SourceEvent<Frame, TelemetryError>),

    /// Completion event for a previously emitted storage effect.
    Stored(EffectOutcome<FrameId, StoreError>),

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
    /// Immutable payload handed to the storage Driver.
    frame: Frame,
}

impl EffectDescriptor for StoreFrame {
    type Output = FrameId;
    type Error = StoreError;
}

/// Application-visible error data reported when the storage Driver fails.
#[derive(Clone, Debug, PartialEq, Eq)]
struct StoreError(String);

/// Telemetry Component definition and its immutable, assembly-supplied wiring.
///
/// The Port belongs here because it is a stable dependency of this Component
/// instance. Mutable feed state belongs in `TelemetryModel`; live connection
/// and decoder state belong in the runtime Driver and framing Layer.
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
) -> impl FnOnce(RequestOutcome<HealthSnapshot>) -> TelemetryMessage + Send + 'static {
    move |event| TelemetryMessage::HealthChecked { probe, event }
}

impl Component for Telemetry {
    type Model = TelemetryModel;
    type Message = TelemetryMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(TelemetryModel::default())
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            TelemetryMessage::Connect(config) => {
                // Changing the Model changes the desired subscription returned
                // below; the runtime performs the actual start or replacement.
                model.desired = Some(config.clone());
                model.last_failure = None;
                Command::notify(self.health.clone(), Connecting(config.endpoint))
            }
            TelemetryMessage::Disconnect => {
                // Removing desire is enough. Reconciliation and cancellation
                // are runtime work, not reducer-side effects.
                model.desired = None;
                Command::notify(self.health.clone(), Disconnected)
            }
            TelemetryMessage::Socket(SourceEvent::Item(frame)) => {
                model.received += 1;
                // One transition can declare independent intents without
                // performing either operation inline.
                Command::batch([
                    Command::notify(self.health.clone(), FrameSeen),
                    Command::effect(StoreFrame { frame }, TelemetryMessage::Stored),
                ])
            }
            TelemetryMessage::Socket(SourceEvent::Failed(error)) => {
                // Failure is terminal. Reconnect policy must arrive as another
                // explicit message with a new desired configuration.
                model.desired = None;
                let error = format!("{error:?}");
                model.last_failure = Some(error.clone());
                Command::notify(self.health.clone(), Failed(error))
            }
            TelemetryMessage::Socket(SourceEvent::Ended) => {
                model.desired = None;
                Command::notify(self.health.clone(), Disconnected)
            }
            TelemetryMessage::Stored(EffectOutcome::Succeeded(_frame_id)) => {
                // Async completion re-enters the Component only as a Message.
                model.stored += 1;
                Command::none()
            }
            TelemetryMessage::Stored(EffectOutcome::Failed(error)) => {
                model.last_failure = Some(error.0);
                Command::none()
            }
            TelemetryMessage::Stored(EffectOutcome::Cancelled(reason)) => {
                model.last_failure = Some(format!("store cancelled: {reason:?}"));
                Command::none()
            }
            TelemetryMessage::CheckHealth(probes) => {
                // Both requests become outstanding from one transition. Completion
                // order is intentionally not assumed; each pure mapper captures
                // the domain key needed to interpret its eventual reply.
                Command::batch(probes.map(|probe| {
                    Command::request(self.health.clone(), Read, move |event| {
                        TelemetryMessage::HealthChecked { probe, event }
                    })
                }))
            }
            TelemetryMessage::HealthChecked { probe, event } => {
                model.health_checks.push((probe, event));
                Command::none()
            }
        }
    }

    fn subscriptions(&self, model: &Self::Model) -> Subscriptions<Self::Message> {
        // Subscriptions are a pure projection of current Model state, analogous
        // to declarative resource desire rather than imperative task spawning.
        let Some(config) = &model.desired else {
            return Subscriptions::none();
        };

        // Reconciliation compares stable identity and the typed descriptor:
        // absence stops it, an equal descriptor preserves it, and a changed
        // descriptor replaces it while keeping the logical subscription
        // identity.
        Subscriptions::one(Subscription::source(
            SubscriptionId::new(SOCKET),
            Framed::new(
                TcpBytes {
                    endpoint: config.endpoint.clone(),
                    generation: config.generation,
                },
                U16LengthDelimited {
                    max_frame_len: config.max_frame_len,
                },
            ),
            TelemetryMessage::Socket,
        ))
    }
}

/// Live Driver for the `StoreFrame` effect boundary.
///
/// Production code would perform Tokio-backed storage here. App retry or
/// classification policy would still be modeled by messages and Components.
struct LiveFrameStore;

impl EffectDriver<StoreFrame> for LiveFrameStore {
    fn execute(
        &self,
        descriptor: StoreFrame,
    ) -> BoxFuture<
        Result<<StoreFrame as EffectDescriptor>::Output, <StoreFrame as EffectDescriptor>::Error>,
    > {
        Box::pin(async move {
            let _ = descriptor;
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

    // Assembly selects the provider; `HealthMessage` declares the canonical
    // Protocol conversion through its `From` implementation.
    program.bind_port(&health_port, &health);

    let telemetry = program.component(
        ComponentId::new("telemetry"),
        Telemetry {
            health: health_port.clone(),
        },
    );

    (
        program.build().expect("the example graph is valid"),
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

/// Shows the live host shape: bind real Drivers, spawn, interact, and perform
/// structured shutdown through the runtime owner.
async fn live_shape() -> Result<(), RuntimeError> {
    let (program, refs) = program();
    let runtime = LiveRuntime::builder(program)
        .bind_source::<TcpBytes, _>(TokioTcpBytes)
        .bind_effect::<StoreFrame, _>(LiveFrameStore)
        .build()?;

    let telemetry = runtime.handle(&refs.telemetry)?;
    let runtime = runtime.spawn();

    // Host interaction still enters through a Message; obtaining a handle does not
    // grant direct access to the Component's Model.
    telemetry
        .send(TelemetryMessage::Connect(config("127.0.0.1:7000", 1)))
        .await?;

    // Runtime-owned work is joined or canceled as one structured lifetime.
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
        TelemetryMessage::Connect(config("simulated:7000", 1)),
    )?;
    runtime.run_until_idle()?;

    // Controlled execution exposes declared runtime state for assertions; it
    // does not require poking at a Driver's private task or socket.
    let socket = SubscriptionId::new(SOCKET);
    let active: TelemetryFeed = runtime.source_descriptor(&refs.telemetry, &socket)?;
    assert_eq!(active.source.endpoint, Endpoint::new("simulated:7000"));

    // Inject raw chunks at the terminal TcpBytes descriptor. The first chunk is
    // partial; the second completes it and contains another whole frame. The
    // same decoder Layer therefore runs in controlled and live execution.
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
    runtime.complete(first, EffectOutcome::Succeeded(FrameId(1)))?;
    runtime.complete(second, EffectOutcome::Succeeded(FrameId(2)))?;
    runtime.run_until_idle()?;
    assert_eq!(runtime.state(&refs.telemetry)?.stored, 2);

    // Same subscription identity, changed resource configuration: replace it.
    runtime.send(
        &refs.telemetry,
        TelemetryMessage::Connect(config("simulated:8000", 2)),
    )?;
    runtime.run_until_idle()?;
    let replaced: TelemetryFeed = runtime.source_descriptor(&refs.telemetry, &socket)?;
    assert_ne!(active, replaced);

    runtime.emit_source::<Telemetry, TcpBytes>(
        &refs.telemetry,
        &socket,
        SourceEvent::Failed(TcpError::Read("connection reset".to_owned())),
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

/// Keeps the binary intentionally inert; the tests exercise both profiles.
fn main() {
    println!("Samara live and controlled reference; see this source and its tests");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Activates the controlled path through Port notification, composed Source
    /// Layers, typed effects, replacement, and failure input.
    #[test]
    fn controlled_reference_program_runs_end_to_end() -> Result<(), RuntimeError> {
        controlled_shape()
    }

    /// Runs the unchanged Components and composed `Framed` descriptor through
    /// real Tokio scheduling and structured Source cancellation. The example's
    /// deliberately pending transport Driver makes Cancel the appropriate host
    /// boundary for this smoke scenario.
    #[tokio::test]
    async fn live_reference_program_runs_end_to_end() -> Result<(), RuntimeError> {
        live_shape().await
    }

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

    /// Exercises the actual composed descriptor used by `Telemetry` without
    /// selecting a live Driver or controlled terminal binding.
    #[test]
    fn framed_layer_maps_reference_transport_events() {
        let descriptor = Framed::new(
            TcpBytes {
                endpoint: Endpoint::new("reference:7000"),
                generation: 1,
            },
            U16LengthDelimited { max_frame_len: 8 },
        );
        let mut layer = descriptor.into_layer();

        assert_eq!(
            layer.map_event(SourceEvent::Item(vec![0, 3, b'a'])),
            Vec::<SourceEvent<Frame, TelemetryError>>::new()
        );
        assert_eq!(
            layer.map_event(SourceEvent::Item(vec![b'b', b'c', 0, 1, b'x'])),
            vec![
                SourceEvent::Item(Frame(b"abc".to_vec())),
                SourceEvent::Item(Frame(b"x".to_vec())),
            ]
        );
        assert_eq!(
            layer.map_event(SourceEvent::Failed(TcpError::Read("reset".to_owned()))),
            vec![SourceEvent::Failed(FramedError::Source(TcpError::Read(
                "reset".to_owned()
            )))]
        );
    }

    /// Proves reconciliation can observe a descriptor replacement even though the
    /// logical subscription retains the same stable identity.
    #[test]
    fn same_subscription_identity_exposes_changed_configuration() {
        let mut builder = Program::builder();
        let health = builder.port(PortId::new(HEALTH));
        let component = Telemetry { health };
        let mut model = component.init().model;
        let id = SubscriptionId::new(SOCKET);

        component.update(&mut model, TelemetryMessage::Connect(config("one:7000", 1)));
        let first = component.subscriptions(&model);
        let first = first.find::<TelemetryFeed>(&id).unwrap();

        component.update(&mut model, TelemetryMessage::Connect(config("two:7000", 2)));
        let second = component.subscriptions(&model);
        let second = second.find::<TelemetryFeed>(&id).unwrap();

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

        builder.bind_port(&primary, &provider);
        builder.bind_port(&fallback, &provider);

        let converted: HealthMessage = HealthProtocolMessage::FrameSeen.into();
        assert!(matches!(
            converted,
            HealthMessage::Protocol(HealthProtocolMessage::FrameSeen)
        ));

        assert_ne!(primary, fallback);
        assert_eq!(primary.id(), &PortId::new("health/primary"));
        assert_eq!(fallback.id(), &PortId::new("health/fallback"));
    }

    /// Proves consumers emit public protocol notifications without importing or
    /// constructing the provider's private `HealthMessage` vocabulary.
    #[test]
    fn telemetry_emits_protocol_notifications_without_provider_messages() {
        let mut builder = Program::builder();
        let health = builder.port(PortId::new(HEALTH));
        let component = Telemetry { health };
        let mut model = component.init().model;

        let command =
            component.update(&mut model, TelemetryMessage::Connect(config("one:7000", 1)));
        let notifications = command.notification_intents::<Connecting>();

        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].0, &PortId::new(HEALTH));
        assert_eq!(notifications[0].1.0, Endpoint::new("one:7000"));
    }

    /// Proves concurrent requests use one result Message variant while preserving
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

        let command = component.update(
            &mut model,
            TelemetryMessage::CheckHealth([ProbeId(17), ProbeId(29)]),
        );
        let requests = command.request_intents::<Read>();

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
