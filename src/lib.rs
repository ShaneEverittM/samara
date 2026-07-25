#![allow(dead_code)]
#![warn(missing_docs)]

//! The compiler-checked candidate for Samara's staged v0 public API.
//!
//! # Status
//!
//! Phase 6 adds structured live Tokio execution alongside deterministic
//! controlled execution. Live tests now interpret typed Commands, reconcile
//! runtime-owned Sources, supervise terminal Drivers and timers, preserve
//! Component serialization and causal delivery, and close the owned scope by
//! explicit Drain or Cancel semantics. Controlled execution continues to own
//! logical time and repeatable program-wide traces.
//!
//! # Mental model
//!
//! 1. A [`Component`] owns a private model and accepts typed messages.
//! 2. [`Component::update`] changes that model synchronously and returns a
//!    [`Command`] describing finite work. It receives no runtime or I/O capability.
//! 3. [`Component::subscriptions`] declares the ongoing [`SourceDescriptor`]
//!    values the current model wants active. Declaring a subscription does not
//!    start work.
//! 4. A [`Program`] assembles Components, typed references, and named [`Port`]
//!    dependencies. Ports expose provider-neutral protocols rather than a
//!    provider Component's private message enum.
//! 5. [`LiveRuntime`] binds effect and source descriptors to Tokio Drivers.
//!    [`ControlledRuntime`] exposes the same boundaries as scripted inputs for
//!    deterministic tests.
//!
//! Commands and subscriptions contain explicit, typed intent. The runtime owns
//! their asynchronous execution and returns behaviorally relevant outcomes as
//! messages through [`EffectOutcome`], [`SourceEvent`], and [`RequestOutcome`].

use std::{
    any::{Any, TypeId},
    error::Error,
    fmt,
    future::Future,
    marker::PhantomData,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    time::Duration,
};

mod component_kernel;
mod controlled_runtime;
mod declarative_work;
mod live_runtime;

use component_kernel::{ComponentKernel, ErasedComponentKernel};
use controlled_runtime::ControlledCore;

/// Convenient imports for writing Components and assembling either runtime
/// profile.
pub mod prelude {
    pub use crate::{
        BoxFuture, CancelReason, Command, Component, ComponentHandle, ComponentId, ComponentRef,
        ControlledRuntime, Decoder, DriverStopped, EffectDescriptor, EffectDriver,
        EffectInvocation, EffectOutcome, EffectOutcomeKind, Framed, FramedError, FramedLayer,
        HttpError, HttpErrorKind, HttpJsonError, HttpRequest, HttpResponse, HttpResponseError,
        HttpStatusError, Init, LiveRuntime, LogicalTime, Notification, PendingEffect, PendingWork,
        Port, PortId, PrintStderr, PrintStdout, Program, ProgramBuildError, ProgramBuilder,
        Protocol, ReplyTo, Request, RequestError, RequestInvocation, RequestOutcome, RunReport,
        RuntimeError, RuntimeTask, Shutdown, ShutdownReport, SourceDescriptor, SourceDriver,
        SourceEvent, SourceEventKind, SourceSink, StreamDescriptor, Subscription,
        SubscriptionAction, SubscriptionId, Subscriptions, TcpBytes, TcpError, TcpErrorKind,
        TraceCommandKind, TraceEvent, TraceId, TraceRecord, protocol,
    };
}

/// Returns a deferred best-effort standard-output [`Command`] using familiar
/// Rust formatting syntax.
///
/// The returned Command owns the formatted text and deliberately discards the
/// print outcome. It must still be returned from the transition or included in
/// [`Command::batch`]. This macro performs no immediate I/O and is intentionally
/// not re-exported by [`prelude`].
#[macro_export]
macro_rules! print {
    () => {
        $crate::Command::effect_discarding_outcome($crate::PrintStdout::text(
            ::std::string::String::new(),
        ))
    };
    ($($argument:tt)+) => {
        $crate::Command::effect_discarding_outcome(
            $crate::PrintStdout::text(::std::format!($($argument)+))
        )
    };
}

/// Returns a deferred best-effort standard-output-line [`Command`] using
/// familiar Rust formatting syntax.
///
/// Exactly one newline is appended after the formatted text. The returned
/// Command owns that text, schedules no completion Message, and must still be
/// returned or batched. This macro is distinct from Rust's ambient,
/// unqualified `println!`.
#[macro_export]
macro_rules! println {
    () => {
        $crate::Command::effect_discarding_outcome($crate::PrintStdout::line(
            ::std::string::String::new(),
        ))
    };
    ($($argument:tt)+) => {
        $crate::Command::effect_discarding_outcome(
            $crate::PrintStdout::line(::std::format!($($argument)+))
        )
    };
}

/// Returns a deferred best-effort standard-error [`Command`] using familiar
/// Rust formatting syntax.
///
/// The returned Command owns the formatted text and deliberately discards the
/// print outcome. It must still be returned from the transition or included in
/// [`Command::batch`]. This macro performs no immediate I/O and is intentionally
/// not re-exported by [`prelude`].
#[macro_export]
macro_rules! eprint {
    () => {
        $crate::Command::effect_discarding_outcome($crate::PrintStderr::text(
            ::std::string::String::new(),
        ))
    };
    ($($argument:tt)+) => {
        $crate::Command::effect_discarding_outcome(
            $crate::PrintStderr::text(::std::format!($($argument)+))
        )
    };
}

/// Returns a deferred best-effort standard-error-line [`Command`] using
/// familiar Rust formatting syntax.
///
/// Exactly one newline is appended after the formatted text. The returned
/// Command owns that text, schedules no completion Message, and must still be
/// returned or batched. This macro is distinct from Rust's ambient,
/// unqualified `eprintln!`.
#[macro_export]
macro_rules! eprintln {
    () => {
        $crate::Command::effect_discarding_outcome($crate::PrintStderr::line(
            ::std::string::String::new(),
        ))
    };
    ($($argument:tt)+) => {
        $crate::Command::effect_discarding_outcome(
            $crate::PrintStderr::line(::std::format!($($argument)+))
        )
    };
}

/// A boxed, sendable future returned by a live effect or source Driver.
///
/// Returning the future to Samara transfers lifecycle ownership to the runtime;
/// Drivers should not detach untracked Tokio tasks behind this boundary.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Stable logical identity of one Component instance in a [`Program`].
///
/// This identifies the application unit, not a Tokio task, mailbox, or thread.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ComponentId(Arc<str>);

impl ComponentId {
    /// Creates a logical Component identity.
    pub fn new(value: impl Into<Arc<str>>) -> Self {
        Self(value.into())
    }
}

/// Stable name of one provider-neutral protocol dependency in a [`Program`].
///
/// A Port's full logical identity is its protocol type plus this name. Two
/// [`Port`] values may therefore use the same [`Protocol`] while naming distinct
/// dependencies, such as `"inventory/primary"` and `"inventory/fallback"`.
/// Neither the name nor registration order implies execution order.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PortId(Arc<str>);

impl PortId {
    /// Creates a stable logical Port name.
    pub fn new(value: impl Into<Arc<str>>) -> Self {
        Self(value.into())
    }
}

/// Stable identity of an ongoing subscription within its owning Component.
///
/// Reconciliation combines this key with the owning [`ComponentId`]. An equal
/// source descriptor keeps the current subscription alive; changed source
/// configuration replaces or reconfigures it according to its contract.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubscriptionId(Arc<str>);

impl SubscriptionId {
    /// Creates a logical subscription identity.
    pub fn new(value: impl Into<Arc<str>>) -> Self {
        Self(value.into())
    }
}

/// A typed description of finite world-facing work.
///
/// Implementations are data only: they do not execute the effect. A live
/// [`EffectDriver`] realizes the value in live execution, while controlled
/// execution exposes it as [`PendingEffect`] for scripted completion.
pub trait EffectDescriptor: Send + 'static {
    /// Value produced when the effect succeeds.
    type Output: Send + 'static;
    /// Typed error explaining an effect failure.
    type Error: Send + 'static;
}

mod source_plan_private {
    /// Unnameable outside this crate, so the hidden lowering hook can be
    /// implemented by downstream descriptors but overridden only by Samara.
    pub struct LowerToken(());

    pub(crate) const LOWER_TOKEN: LowerToken = LowerToken(());
}

/// A cloneable, comparable description of an ongoing external event source.
///
/// Equality is semantic: the runtime uses it during subscription reconciliation
/// to decide whether an active source is unchanged. Operational state such as a
/// socket handle or partial input buffer must not live in this descriptor.
///
/// SourcePlan lowering is reserved to Samara. A downstream descriptor supplies
/// only its event types and inherits terminal behavior; the hidden dispatch
/// method cannot be overridden without naming crate-private types:
///
/// ```compile_fail
/// use samara::SourceDescriptor;
///
/// #[derive(Clone, Debug, PartialEq)]
/// struct Custom;
///
/// impl SourceDescriptor for Custom {
///     type Item = ();
///     type Error = ();
///
///     fn __samara_source_plan(
///         &self,
///         _: samara::source_plan_private::LowerToken,
///     ) -> samara::SourcePlan {
///         unreachable!()
///     }
/// }
/// ```
pub trait SourceDescriptor: Clone + PartialEq + fmt::Debug + Send + Sync + 'static {
    /// Item emitted while the source remains active.
    type Item: Send + 'static;
    /// Typed error explaining a failure that terminates the active Source.
    type Error: Send + 'static;

    /// Internal stable-Rust dispatch for Samara's built-in composed descriptors.
    ///
    /// The token and result are crate-private, making this a partially sealed
    /// implementation hook rather than a downstream Layer extension point.
    /// Application descriptors inherit the terminal default; only Samara can
    /// name the token required to override it.
    #[doc(hidden)]
    #[allow(private_interfaces)]
    fn __samara_source_plan(&self, _: source_plan_private::LowerToken) -> SourcePlan {
        SourcePlan::terminal(self.clone())
    }
}

/// Runtime-owned lowering of one composed [`SourceDescriptor`].
pub(crate) struct SourcePlan {
    terminal: Box<dyn ErasedTerminalDescriptor>,
    layers: Vec<Box<dyn ErasedSourceLayer>>,
    output_event_type: TypeId,
    valid_event_chain: bool,
}

trait ErasedTerminalDescriptor: Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn descriptor_type_id(&self) -> TypeId;
    fn type_name(&self) -> &'static str;
}

struct TerminalDescriptor<S: SourceDescriptor>(S);

impl<S: SourceDescriptor> ErasedTerminalDescriptor for TerminalDescriptor<S> {
    fn as_any(&self) -> &dyn Any {
        &self.0
    }

    fn descriptor_type_id(&self) -> TypeId {
        TypeId::of::<S>()
    }

    fn type_name(&self) -> &'static str {
        std::any::type_name::<S>()
    }
}

trait ErasedSourceLayer: Send {
    fn map_event(&mut self, event: ErasedSourceEvent) -> Vec<ErasedSourceEvent>;
}

pub(crate) struct ErasedSourceEvent {
    value: Box<dyn Any + Send>,
    kind: SourceEventKind,
}

impl ErasedSourceEvent {
    pub(crate) fn typed<S: SourceDescriptor>(event: SourceEvent<S::Item, S::Error>) -> Self {
        let kind = SourceEventKind::of(&event);
        Self {
            value: Box::new(event),
            kind,
        }
    }
}

impl SourcePlan {
    fn terminal<S: SourceDescriptor>(descriptor: S) -> Self {
        Self {
            terminal: Box::new(TerminalDescriptor(descriptor)),
            layers: Vec::new(),
            output_event_type: TypeId::of::<SourceEvent<S::Item, S::Error>>(),
            valid_event_chain: true,
        }
    }

    pub(crate) fn terminal_type_id(&self) -> TypeId {
        self.terminal.descriptor_type_id()
    }

    pub(crate) fn terminal_type_name(&self) -> &'static str {
        self.terminal.type_name()
    }

    pub(crate) fn terminal_descriptor(&self) -> &dyn Any {
        self.terminal.as_any()
    }

    pub(crate) fn accepts_output_event_type(&self, expected: TypeId) -> bool {
        self.valid_event_chain && self.output_event_type == expected
    }

    pub(crate) fn map_event(&mut self, event: ErasedSourceEvent) -> Vec<ErasedSourceEvent> {
        self.layers.iter_mut().fold(vec![event], |events, layer| {
            events
                .into_iter()
                .flat_map(|event| layer.map_event(event))
                .collect()
        })
    }
}

/// Behaviorally relevant terminal outcome of one finite [`EffectDescriptor`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EffectOutcome<Output, EffectError> {
    /// The effect completed with its typed output.
    Succeeded(Output),
    /// The effect completed with its typed failure.
    Failed(EffectError),
    /// Runtime ownership ended the effect before completion.
    Cancelled(CancelReason),
}

/// Event delivered by an active Source realization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceEvent<Item, SourceError> {
    /// The source emitted another typed item and remains active.
    Item(Item),
    /// The active source instance failed and terminated.
    Failed(SourceError),
    /// The active source ended normally.
    Ended,
}

/// Structural shape of one [`SourceEvent`] in the controlled trace.
///
/// Payloads remain available to typed Component tests and are deliberately not
/// copied into the generic v0 trace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceEventKind {
    /// The Source emitted another item and remains active.
    Item,
    /// The Source failed and terminated.
    Failed,
    /// The Source ended normally.
    Ended,
}

impl SourceEventKind {
    fn of<Item, SourceError>(event: &SourceEvent<Item, SourceError>) -> Self {
        match event {
            SourceEvent::Item(_) => Self::Item,
            SourceEvent::Failed(_) => Self::Failed,
            SourceEvent::Ended => Self::Ended,
        }
    }

    fn is_terminal(self) -> bool {
        matches!(self, Self::Failed | Self::Ended)
    }
}

/// Runtime-owned reason why finite work did not complete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CancelReason {
    /// The owning runtime or Component scope shut down.
    Shutdown,
    /// Newer intent made the pending work obsolete.
    Superseded,
    /// The effect did not complete before its declared deadline.
    Deadline,
}

/// Initial model and optional startup command for a [`Component`].
pub struct Init<Model, Message> {
    /// The Component's initial private state.
    pub model: Model,
    /// Finite work requested as the Component enters the program.
    pub command: Command<Message>,
}

impl<Model, Message> Init<Model, Message> {
    /// Creates initialization with no startup command.
    pub fn new(model: Model) -> Self {
        Self {
            model,
            command: Command::none(),
        }
    }

    /// Adds finite work to request after the initial model is installed.
    pub fn with_command(mut self, command: Command<Message>) -> Self {
        self.command = command;
        self
    }
}

/// Samara's topology-neutral unit of state and behavior.
///
/// A Component configuration may contain immutable logical wiring such as an
/// [`StreamDescriptor`], [`Port`], or [`ComponentRef`], but it must not contain ambient
/// runtime, clock, I/O, or mutable state handles. The runtime owns `Model` and
/// guarantees that two calls to [`Component::update`] for the same Component
/// never overlap.
///
/// The associated Message type makes every transition input explicit. Values
/// from another vocabulary cannot be delivered accidentally:
///
/// ```compile_fail
/// use samara::{Command, Component};
///
/// struct OtherMessage;
///
/// fn deliver_wrong_type<C: Component>(
///     component: &C,
///     model: &mut C::Model,
///     message: OtherMessage,
/// ) -> Command<C::Message> {
///     component.update(model, message)
/// }
/// ```
pub trait Component: Send + 'static {
    /// All mutable application state exclusively owned by this Component.
    type Model: Send + 'static;
    /// The only input allowed to trigger a state transition.
    type Message: Send + 'static;

    /// Produces the initial model and any explicit startup work.
    fn init(&self) -> Init<Self::Model, Self::Message>;

    /// Must be observationally pure. The mutable reference is exclusively owned
    /// for the duration of this non-overlapping transition.
    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message>;

    /// Purely describes the ongoing sources desired by the current model.
    ///
    /// The runtime evaluates this after a committed transition and reconciles
    /// the returned set by [`SubscriptionId`] plus source equality. The default
    /// declares no ongoing work.
    fn subscriptions(&self, _model: &Self::Model) -> Subscriptions<Self::Message> {
        Subscriptions::none()
    }
}

/// A provider-neutral set of values accepted through a [`Port`].
///
/// `Message` is normally a protocol-owned enum containing notification values
/// and typed [`RequestInvocation`] requests. It is deliberately distinct from
/// any provider Component's private [`Component::Message`] type. Program
/// assembly converts it into the selected provider's Message through the
/// `From` relationship required by [`ProgramBuilder::bind_port`].
pub trait Protocol: Send + Sync + 'static {
    /// Complete provider-facing input vocabulary for this protocol.
    type Message: Send + 'static;
}

/// A one-way value belonging to protocol `P`.
///
/// Conversion to [`Protocol::Message`] is synchronous application logic and
/// must be pure and deterministic. Sending the returned value remains a
/// runtime-interpreted command rather than a direct provider call.
pub trait Notification<P: Protocol>: Send + 'static {
    /// Wraps this notification in the protocol's provider-facing vocabulary.
    fn into_message(self) -> P::Message;
}

/// A request belonging to protocol `P` with one statically known reply type.
///
/// The runtime creates an opaque [`ReplyTo`] value when interpreting
/// [`Command::request`]. This pure conversion places the request and token into
/// the protocol's [`Protocol::Message`] vocabulary for delivery to its bound
/// provider.
pub trait Request<P: Protocol>: Send + 'static {
    /// Reply value accepted for this particular request type.
    type Reply: Send + 'static;

    /// Wraps the request and runtime-owned reply token for provider delivery.
    fn into_message(self, reply_to: ReplyTo<Self::Reply>) -> P::Message;
}

/// Requester-visible terminal outcome of a correlated [`Command::request`].
///
/// The variants establish that request liveness is explicit Component input. The
/// default timeout policy, cancellation taxonomy, and exact point at which a
/// delivery becomes failed remain deliberately unresolved for this API slice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestOutcome<Reply> {
    /// The provider emitted a correctly typed reply.
    Replied(Reply),
    /// The request or reply could not complete for a runtime-visible reason.
    Failed(RequestError),
    /// A configured or runtime-owned deadline expired before a reply arrived.
    TimedOut,
    /// Runtime ownership ended the request for a reason other than timeout.
    Cancelled,
}

/// Provisional topology-neutral error reported through [`RequestOutcome`].
///
/// These variants distinguish delivery from abandoned-reply failures without
/// committing to mailboxes, channels, tasks, or a particular provider failure
/// detector. The production taxonomy is still an API design question.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RequestError {
    /// The bound provider could not accept the request.
    DeliveryFailed,
    /// The provider accepted the request but no reply can now be produced.
    ReplyAbandoned,
}

/// Opaque, one-shot correlation authority carried to a request provider.
///
/// This is inert typed data: it is not a oneshot sender, future, runtime handle,
/// or state mutation capability. It deliberately does not implement `Clone`,
/// so [`Command::reply`] consumes the only authority presented to the provider.
/// Application code cannot construct a token; the runtime creates it while
/// interpreting [`Command::request`].
///
/// Discarding the authority is a diagnostic violation when `unused_must_use`
/// is denied. This lint catches immediate accidental discards; runtime-owned
/// lifecycle diagnostics must still detect obligations abandoned after being
/// stored or deliberately forgotten.
///
/// ```compile_fail
/// #![deny(unused_must_use)]
/// use samara::ReplyTo;
///
/// fn discard(reply_to: ReplyTo<()>) {
///     reply_to;
/// }
/// ```
#[must_use = "a ReplyTo must be consumed by Command::reply"]
pub struct ReplyTo<Reply> {
    correlation: u64,
    marker: PhantomData<fn(Reply)>,
}

impl<Reply> ReplyTo<Reply> {
    fn sketch(correlation: u64) -> Self {
        Self {
            correlation,
            marker: PhantomData,
        }
    }

    fn runtime(correlation: u64) -> Self {
        Self::sketch(correlation)
    }
}

impl<Reply> fmt::Debug for ReplyTo<Reply> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = self.correlation;
        formatter.debug_tuple("ReplyTo").field(&"<opaque>").finish()
    }
}

/// One typed request delivered to a protocol provider.
///
/// Protocol definitions normally place this value in a
/// [`Protocol::Message`] enum variant. Provider Components inspect `request`
/// and pass `reply_to` to [`Command::reply`]; they never await or access runtime
/// state while handling it.
pub struct RequestInvocation<P, R>
where
    P: Protocol,
    R: Request<P>,
{
    /// Original typed request value supplied by the requester.
    pub request: R,
    /// Opaque authority for emitting exactly `R::Reply` to that requester.
    pub reply_to: ReplyTo<R::Reply>,
    protocol: PhantomData<fn() -> P>,
}

impl<P, R> RequestInvocation<P, R>
where
    P: Protocol,
    R: Request<P>,
{
    /// Couples a request to the runtime-created reply authority.
    ///
    /// Protocol-owned [`Request::into_message`] implementations call this; the
    /// private construction of [`ReplyTo`] prevents application code from
    /// manufacturing a live correlation.
    pub fn new(request: R, reply_to: ReplyTo<R::Reply>) -> Self {
        Self {
            request,
            reply_to,
            protocol: PhantomData,
        }
    }
}

impl<P, R> fmt::Debug for RequestInvocation<P, R>
where
    P: Protocol,
    R: Request<P> + fmt::Debug,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RequestInvocation")
            .field("request", &self.request)
            .field("reply_to", &self.reply_to)
            .finish()
    }
}

/// Declares the operation vocabulary of a provider-neutral protocol.
///
/// Each entry generates an ordinary, nameable operation type. The declaration
/// also generates a marker type, its [`Protocol`] implementation, a
/// provider-facing protocol-message enum, and the corresponding
/// [`Notification`] and [`Request`] conversions. Provider Components still
/// match the generated protocol-message enum themselves.
///
/// This is deliberately a small `macro_rules!` experiment. An entry with no
/// reply type is a notification; an entry followed by `-> Reply` is a request.
/// Entries may be unit operations or carry one tuple field. Notification
/// variants are flattened to that field, while request variants carry a typed
/// [`RequestInvocation`]. A request with a unit reply must therefore spell
/// `-> ()`. Generated operation types and the protocol-message enum currently derive
/// [`Debug`](fmt::Debug).
///
/// The prototype does not yet accept per-operation attributes, multiple
/// fields, generics, or configurable derives. Those remain API questions; the
/// narrow grammar is not intended to settle them.
///
/// ```
/// use samara::prelude::*;
///
/// #[derive(Debug)]
/// pub struct HealthSnapshot;
///
/// protocol! {
///     pub type HealthProtocol => enum HealthProtocolMessage {
///         Changed(String),
///         Disconnected,
///         Read -> HealthSnapshot,
///     }
/// }
///
/// let changed = <Changed as Notification<HealthProtocol>>::into_message(
///     Changed("receiving".to_owned()),
/// );
/// assert!(matches!(changed, HealthProtocolMessage::Changed(status) if status == "receiving"));
/// ```
#[macro_export]
macro_rules! protocol {
    (
        $(#[$protocol_attr:meta])*
        $visibility:vis type $protocol:ident => enum $message:ident {
            $($entries:tt)*
        }
    ) => {
        $crate::protocol! {
            @parse
            [$(#[$protocol_attr])*]
            [$visibility]
            [$protocol]
            [$message]
            []
            []
            []
            $($entries)*
        }
    };

    // Finishes the TT muncher after accumulating generated operation items,
    // protocol-message variants, and trait implementations. Building the complete enum
    // here avoids relying on nested macros to expand to individual variants.
    (
        @parse
        [$(#[$protocol_attr:meta])*]
        [$visibility:vis]
        [$protocol:ident]
        [$message:ident]
        [$($operation_items:tt)*]
        [$($message_variants:tt)*]
        [$($implementations:tt)*]
    ) => {
        $(#[$protocol_attr])*
        $visibility struct $protocol;

        impl $crate::Protocol for $protocol {
            type Message = $message;
        }

        $($operation_items)*

        #[doc = concat!("Provider-facing envelope for `", stringify!($protocol), "`.")]
        #[derive(Debug)]
        $visibility enum $message {
            $($message_variants)*
        }

        $($implementations)*
    };

    // A payload-carrying request generates a tuple operation type, while its
    // protocol-message variant carries the operation plus correlation token.
    (
        @parse
        [$($protocol_attr:tt)*]
        [$visibility:vis]
        [$protocol:ident]
        [$message:ident]
        [$($operation_items:tt)*]
        [$($message_variants:tt)*]
        [$($implementations:tt)*]
        $operation:ident($payload:ty) -> $reply:ty,
        $($remaining:tt)*
    ) => {
        $crate::protocol! {
            @parse
            [$($protocol_attr)*]
            [$visibility]
            [$protocol]
            [$message]
            [
                $($operation_items)*
                #[doc = concat!("Payload-carrying request operation in `", stringify!($protocol), "`.")]
                #[derive(Debug)]
                $visibility struct $operation(
                    #[doc = "Request payload."]
                    $visibility $payload
                );
            ]
            [
                $($message_variants)*
                #[doc = concat!("Request operation `", stringify!($operation), "`.")]
                $operation($crate::RequestInvocation<$protocol, $operation>),
            ]
            [
                $($implementations)*
                impl $crate::Request<$protocol> for $operation {
                    type Reply = $reply;

                    fn into_message(
                        self,
                        reply_to: $crate::ReplyTo<Self::Reply>,
                    ) -> $message {
                        $message::$operation($crate::RequestInvocation::new(self, reply_to))
                    }
                }
            ]
            $($remaining)*
        }
    };

    // A unit request still has a tuple protocol-message variant because
    // `RequestInvocation`
    // carries the runtime-created reply authority.
    (
        @parse
        [$($protocol_attr:tt)*]
        [$visibility:vis]
        [$protocol:ident]
        [$message:ident]
        [$($operation_items:tt)*]
        [$($message_variants:tt)*]
        [$($implementations:tt)*]
        $operation:ident -> $reply:ty,
        $($remaining:tt)*
    ) => {
        $crate::protocol! {
            @parse
            [$($protocol_attr)*]
            [$visibility]
            [$protocol]
            [$message]
            [
                $($operation_items)*
                #[doc = concat!("Unit request operation in `", stringify!($protocol), "`.")]
                #[derive(Debug)]
                $visibility struct $operation;
            ]
            [
                $($message_variants)*
                #[doc = concat!("Request operation `", stringify!($operation), "`.")]
                $operation($crate::RequestInvocation<$protocol, $operation>),
            ]
            [
                $($implementations)*
                impl $crate::Request<$protocol> for $operation {
                    type Reply = $reply;

                    fn into_message(
                        self,
                        reply_to: $crate::ReplyTo<Self::Reply>,
                    ) -> $message {
                        $message::$operation($crate::RequestInvocation::new(self, reply_to))
                    }
                }
            ]
            $($remaining)*
        }
    };

    // A payload notification is flattened in the provider-facing enum: the
    // generated operation is the sender-side value, not another wrapper layer.
    (
        @parse
        [$($protocol_attr:tt)*]
        [$visibility:vis]
        [$protocol:ident]
        [$message:ident]
        [$($operation_items:tt)*]
        [$($message_variants:tt)*]
        [$($implementations:tt)*]
        $operation:ident($payload:ty),
        $($remaining:tt)*
    ) => {
        $crate::protocol! {
            @parse
            [$($protocol_attr)*]
            [$visibility]
            [$protocol]
            [$message]
            [
                $($operation_items)*
                #[doc = concat!("Payload-carrying notification operation in `", stringify!($protocol), "`.")]
                #[derive(Debug)]
                $visibility struct $operation(
                    #[doc = "Notification payload."]
                    $visibility $payload
                );
            ]
            [
                $($message_variants)*
                #[doc = concat!("Notification operation `", stringify!($operation), "`.")]
                $operation($payload),
            ]
            [
                $($implementations)*
                impl $crate::Notification<$protocol> for $operation {
                    fn into_message(self) -> $message {
                        let $operation(payload) = self;
                        $message::$operation(payload)
                    }
                }
            ]
            $($remaining)*
        }
    };

    // A unit notification becomes a unit protocol-message variant.
    (
        @parse
        [$($protocol_attr:tt)*]
        [$visibility:vis]
        [$protocol:ident]
        [$message:ident]
        [$($operation_items:tt)*]
        [$($message_variants:tt)*]
        [$($implementations:tt)*]
        $operation:ident,
        $($remaining:tt)*
    ) => {
        $crate::protocol! {
            @parse
            [$($protocol_attr)*]
            [$visibility]
            [$protocol]
            [$message]
            [
                $($operation_items)*
                #[doc = concat!("Unit notification operation in `", stringify!($protocol), "`.")]
                #[derive(Debug)]
                $visibility struct $operation;
            ]
            [
                $($message_variants)*
                #[doc = concat!("Notification operation `", stringify!($operation), "`.")]
                $operation,
            ]
            [
                $($implementations)*
                impl $crate::Notification<$protocol> for $operation {
                    fn into_message(self) -> $message {
                        $message::$operation
                    }
                }
            ]
            $($remaining)*
        }
    };
}

trait ErasedEffectCommand<Message>: Send {
    fn intent(&self) -> &dyn Any;
    fn intent_type_name(&self) -> &'static str;
    fn maps_outcome(&self) -> bool;
    fn into_parts(self: Box<Self>) -> (Box<dyn Any + Send>, Option<ErasedEffectMapper<Message>>);
}

type ErasedEffectMapper<Message> = Box<dyn FnOnce(Box<dyn Any + Send>) -> Message + Send + 'static>;

type EffectMapper<E, Message> = Box<
    dyn FnOnce(
            EffectOutcome<<E as EffectDescriptor>::Output, <E as EffectDescriptor>::Error>,
        ) -> Message
        + Send
        + 'static,
>;

struct Perform<E, Map> {
    effect: E,
    map: Map,
}

impl<Message, E, Map> ErasedEffectCommand<Message> for Perform<E, Map>
where
    Message: Send + 'static,
    E: EffectDescriptor,
    Map: FnOnce(EffectOutcome<E::Output, E::Error>) -> Message + Send + 'static,
{
    fn intent(&self) -> &dyn Any {
        &self.effect
    }

    fn intent_type_name(&self) -> &'static str {
        std::any::type_name::<E>()
    }

    fn maps_outcome(&self) -> bool {
        true
    }

    fn into_parts(self: Box<Self>) -> (Box<dyn Any + Send>, Option<ErasedEffectMapper<Message>>) {
        let Self { effect, map } = *self;
        let mapper = Box::new(move |outcome: Box<dyn Any + Send>| {
            let outcome = match outcome.downcast::<EffectOutcome<E::Output, E::Error>>() {
                Ok(outcome) => *outcome,
                Err(_) => {
                    unreachable!("effect type and outcome type are coupled by EffectDescriptor")
                }
            };
            map(outcome)
        });

        (Box::new(effect), Some(mapper))
    }
}

struct PerformDiscardingOutcome<E> {
    effect: E,
}

impl<Message, E> ErasedEffectCommand<Message> for PerformDiscardingOutcome<E>
where
    E: EffectDescriptor,
{
    fn intent(&self) -> &dyn Any {
        &self.effect
    }

    fn intent_type_name(&self) -> &'static str {
        std::any::type_name::<E>()
    }

    fn maps_outcome(&self) -> bool {
        false
    }

    fn into_parts(self: Box<Self>) -> (Box<dyn Any + Send>, Option<ErasedEffectMapper<Message>>) {
        (Box::new(self.effect), None)
    }
}

trait ErasedSendCommand: Send {
    fn target(&self) -> &ComponentId;
    fn target_program(&self) -> &Arc<()>;
    fn message_type_id(&self) -> TypeId;
    fn message_type_name(&self) -> &'static str;
    fn into_message(self: Box<Self>) -> Box<dyn Any + Send>;
}

struct SendTo<C: Component> {
    target: ComponentRef<C>,
    message: C::Message,
}

impl<C: Component> ErasedSendCommand for SendTo<C> {
    fn target(&self) -> &ComponentId {
        self.target.id()
    }

    fn target_program(&self) -> &Arc<()> {
        &self.target.program
    }

    fn message_type_name(&self) -> &'static str {
        std::any::type_name::<C::Message>()
    }

    fn message_type_id(&self) -> TypeId {
        TypeId::of::<C::Message>()
    }

    fn into_message(self: Box<Self>) -> Box<dyn Any + Send> {
        Box::new(self.message)
    }
}

trait ErasedNotificationCommand: Send {
    fn port(&self) -> &PortId;
    fn port_program(&self) -> &Arc<()>;
    fn notification(&self) -> &dyn Any;
    fn notification_type_name(&self) -> &'static str;
    fn protocol_type_id(&self) -> TypeId;
    fn protocol_type_name(&self) -> &'static str;
    fn into_message(self: Box<Self>) -> Box<dyn Any + Send>;
}

struct Notify<P, N>
where
    P: Protocol,
    N: Notification<P>,
{
    port: Port<P>,
    notification: N,
}

impl<P, N> ErasedNotificationCommand for Notify<P, N>
where
    P: Protocol,
    N: Notification<P>,
{
    fn port(&self) -> &PortId {
        self.port.id()
    }

    fn port_program(&self) -> &Arc<()> {
        &self.port.program
    }

    fn notification(&self) -> &dyn Any {
        &self.notification
    }

    fn notification_type_name(&self) -> &'static str {
        std::any::type_name::<N>()
    }

    fn protocol_type_id(&self) -> TypeId {
        TypeId::of::<P>()
    }

    fn protocol_type_name(&self) -> &'static str {
        std::any::type_name::<P>()
    }

    fn into_message(self: Box<Self>) -> Box<dyn Any + Send> {
        Box::new(self.notification.into_message())
    }
}

trait ErasedRequestCommand<Message>: Send {
    fn port(&self) -> &PortId;
    fn port_program(&self) -> &Arc<()>;
    fn request(&self) -> &dyn Any;
    fn request_type_name(&self) -> &'static str;
    fn reply_type_name(&self) -> &'static str;
    fn reply_type_id(&self) -> TypeId;
    fn protocol_type_id(&self) -> TypeId;
    fn protocol_type_name(&self) -> &'static str;
    fn mapper_type_name(&self) -> &'static str;
    fn into_parts(
        self: Box<Self>,
        correlation: u64,
    ) -> (Box<dyn Any + Send>, ErasedRequestMapper<Message>);
}

type ErasedRequestMapper<Message> =
    Box<dyn FnOnce(Box<dyn Any + Send>) -> Message + Send + 'static>;

struct RequestCommand<P, R, Map>
where
    P: Protocol,
    R: Request<P>,
{
    port: Port<P>,
    request: R,
    map: Map,
}

impl<Message, P, R, Map> ErasedRequestCommand<Message> for RequestCommand<P, R, Map>
where
    Message: Send + 'static,
    P: Protocol,
    R: Request<P>,
    Map: FnOnce(RequestOutcome<R::Reply>) -> Message + Send + 'static,
{
    fn port(&self) -> &PortId {
        self.port.id()
    }

    fn port_program(&self) -> &Arc<()> {
        &self.port.program
    }

    fn request(&self) -> &dyn Any {
        &self.request
    }

    fn request_type_name(&self) -> &'static str {
        std::any::type_name::<R>()
    }

    fn reply_type_name(&self) -> &'static str {
        std::any::type_name::<R::Reply>()
    }

    fn reply_type_id(&self) -> TypeId {
        TypeId::of::<R::Reply>()
    }

    fn protocol_type_id(&self) -> TypeId {
        TypeId::of::<P>()
    }

    fn protocol_type_name(&self) -> &'static str {
        std::any::type_name::<P>()
    }

    fn mapper_type_name(&self) -> &'static str {
        std::any::type_name::<Map>()
    }

    fn into_parts(
        self: Box<Self>,
        correlation: u64,
    ) -> (Box<dyn Any + Send>, ErasedRequestMapper<Message>) {
        let Self { request, map, .. } = *self;
        let message = request.into_message(ReplyTo::runtime(correlation));
        let mapper = Box::new(move |reply: Box<dyn Any + Send>| {
            let reply = match reply.downcast::<R::Reply>() {
                Ok(reply) => *reply,
                Err(_) => unreachable!("Request couples correlation to its Reply type"),
            };
            map(RequestOutcome::Replied(reply))
        });
        (Box::new(message), mapper)
    }
}

trait ErasedReplyCommand: Send {
    fn reply(&self) -> &dyn Any;
    fn reply_type_name(&self) -> &'static str;
    fn reply_type_id(&self) -> TypeId;
    fn into_parts(self: Box<Self>) -> (u64, Box<dyn Any + Send>);
}

struct Reply<Reply> {
    reply_to: ReplyTo<Reply>,
    reply: Reply,
}

/// One typed effect occurrence intercepted from an inert [`Command`].
///
/// The invocation owns the concrete descriptor without requiring it to be
/// cloneable and retains the matching one-shot Message mapper. Creating this
/// value performs no world interaction. A later execution profile can move the
/// descriptor to terminal behavior while retaining the mapper under
/// runtime-owned correlation. Effects created by
/// [`Command::effect_discarding_outcome`] have no mapper and are inspected
/// through [`Command::effect_intent`] instead.
pub struct EffectInvocation<E, Message>
where
    E: EffectDescriptor,
{
    descriptor: E,
    mapper: EffectMapper<E, Message>,
}

impl<E, Message> EffectInvocation<E, Message>
where
    E: EffectDescriptor,
{
    /// Borrows the explicit descriptor for inspection before execution.
    pub fn descriptor(&self) -> &E {
        &self.descriptor
    }

    /// Applies the stored pure mapper to the invocation's sole terminal outcome.
    ///
    /// Consuming the invocation makes a second mapper call impossible:
    ///
    /// ```compile_fail
    /// use samara::{Command, EffectDescriptor, EffectOutcome};
    ///
    /// struct Read;
    /// impl EffectDescriptor for Read {
    ///     type Output = ();
    ///     type Error = ();
    /// }
    ///
    /// let invocation = Command::effect_with(Read, |_| ())
    ///     .into_effect::<Read>()
    ///     .ok()
    ///     .unwrap();
    /// invocation.map_outcome(EffectOutcome::Succeeded(()));
    /// invocation.map_outcome(EffectOutcome::Succeeded(()));
    /// ```
    pub fn map_outcome(self, outcome: EffectOutcome<E::Output, E::Error>) -> Message {
        (self.mapper)(outcome)
    }

    pub(crate) fn into_parts(self) -> (E, EffectMapper<E, Message>) {
        (self.descriptor, self.mapper)
    }
}

impl<ReplyValue> ErasedReplyCommand for Reply<ReplyValue>
where
    ReplyValue: Send + 'static,
{
    fn reply(&self) -> &dyn Any {
        &self.reply
    }

    fn reply_type_name(&self) -> &'static str {
        std::any::type_name::<ReplyValue>()
    }

    fn reply_type_id(&self) -> TypeId {
        TypeId::of::<ReplyValue>()
    }

    fn into_parts(self: Box<Self>) -> (u64, Box<dyn Any + Send>) {
        (self.reply_to.correlation, Box::new(self.reply))
    }
}

/// A value describing finite work; never the work itself.
///
/// `Command<Message>` can hold heterogeneous typed [`EffectDescriptor`] values
/// because the runtime preserves their concrete types internally. Effect
/// message mappers are synchronous application logic and must be pure and
/// deterministic. Mapper object identity is not part of command semantics.
///
/// Commands intentionally do not implement `Clone`, `Debug`, or `PartialEq`:
/// they may contain one-shot message mappers. Transition tests inspect concrete
/// intent through [`Command::effect_intents`],
/// [`Command::notification_intents`], and [`Command::request_intents`].
/// [`Command::into_effect`] then exposes the owned descriptor and its mapper for
/// direct conformance tests.
///
/// Discarding a Command is a diagnostic violation when `unused_must_use` is
/// denied because constructing inert intent does not submit it to a runtime:
///
/// ```compile_fail
/// #![deny(unused_must_use)]
/// use samara::Command;
///
/// fn discard() {
///     let command: Command<()> = Command::none();
///     command;
/// }
/// ```
#[must_use = "commands are inert declarations; return, batch, or interpret this Command for it to take effect"]
pub struct Command<Message>(CommandKind<Message>);

enum CommandKind<Message> {
    None,
    Effect(Box<dyn ErasedEffectCommand<Message>>),
    Send(Box<dyn ErasedSendCommand>),
    Notify(Box<dyn ErasedNotificationCommand>),
    Request(Box<dyn ErasedRequestCommand<Message>>),
    Reply(Box<dyn ErasedReplyCommand>),
    After { delay: Duration, message: Message },
    Batch(Vec<Command<Message>>),
}

impl<Message> Command<Message> {
    /// Requests no finite work.
    pub fn none() -> Self {
        Self(CommandKind::None)
    }

    /// Combines a typed effect intent with its canonical Message conversion.
    ///
    /// This short form uses `Message: From<EffectOutcome<...>>`. Use
    /// [`Command::effect_with`] when this occurrence must capture domain
    /// context or map the same outcome type differently from another call
    /// site.
    pub fn effect<E>(effect: E) -> Self
    where
        Message: From<EffectOutcome<E::Output, E::Error>> + Send + 'static,
        E: EffectDescriptor,
    {
        Self::effect_with(effect, Message::from)
    }

    /// Combines a typed effect intent with an explicit pure message mapper.
    ///
    /// Live execution passes `effect` to the registered [`EffectDriver<E>`].
    /// Controlled execution exposes it through
    /// [`ControlledRuntime::next_effect`] and invokes `map` when
    /// [`ControlledRuntime::complete`] supplies an [`EffectOutcome`].
    ///
    /// An arbitrary value cannot stand in for an explicit descriptor:
    ///
    /// ```compile_fail
    /// use samara::Command;
    ///
    /// struct HiddenWork;
    /// let _: Command<()> = Command::effect_with(HiddenWork, |_| ());
    /// ```
    ///
    /// The message mapper is synchronous application logic rather than async
    /// work hidden behind the Command boundary:
    ///
    /// ```compile_fail
    /// use samara::{Command, EffectDescriptor};
    ///
    /// struct Read;
    /// impl EffectDescriptor for Read {
    ///     type Output = ();
    ///     type Error = ();
    /// }
    ///
    /// let _: Command<()> = Command::effect_with(Read, |_| async {});
    /// ```
    pub fn effect_with<E, Map>(effect: E, map: Map) -> Self
    where
        Message: Send + 'static,
        E: EffectDescriptor,
        Map: FnOnce(EffectOutcome<E::Output, E::Error>) -> Message + Send + 'static,
    {
        Self(CommandKind::Effect(Box::new(Perform { effect, map })))
    }

    /// Requests a typed finite effect without an application continuation.
    ///
    /// This discards only the terminal [`EffectOutcome`]; it does not detach
    /// the work. Live Drain still waits for the Driver, Cancel still aborts its
    /// runtime-owned task, and controlled execution still exposes the
    /// descriptor through [`ControlledRuntime::next_effect`] until the harness
    /// completes or cancels it. Accepted outcomes remain structurally traced
    /// but schedule no Message.
    pub fn effect_discarding_outcome<E>(effect: E) -> Self
    where
        E: EffectDescriptor,
    {
        Self(CommandKind::Effect(Box::new(PerformDiscardingOutcome {
            effect,
        })))
    }

    /// Requests one-way delivery to another Component.
    ///
    /// This is cross-Component effect intent, not a direct method call. The
    /// target's transition occurs later through normal message delivery.
    /// Delivery failure is diagnostic-only in this provisional lower-level
    /// shape. Prefer [`Command::notify`] for a provider-neutral one-way protocol
    /// and [`Command::request`] when the sender needs a typed terminal outcome.
    pub fn send<C>(target: ComponentRef<C>, message: C::Message) -> Self
    where
        C: Component,
    {
        Self(CommandKind::Send(Box::new(SendTo { target, message })))
    }

    /// Requests one-way delivery through a named provider-neutral [`Port`].
    ///
    /// The sender depends on `P` and notification `N`, not the bound provider's
    /// private [`Component::Message`] enum. The runtime converts `N` through
    /// [`Notification::into_message`] and routes it using the exact Port binding
    /// declared by [`ProgramBuilder::bind_port`]. No provider transition occurs
    /// while this command is constructed.
    pub fn notify<P, N>(port: Port<P>, notification: N) -> Self
    where
        P: Protocol,
        N: Notification<P>,
    {
        Self(CommandKind::Notify(Box::new(Notify { port, notification })))
    }

    /// Sends a correlated request using its canonical Message conversion.
    ///
    /// This short form uses `Message: From<RequestOutcome<R::Reply>>`. Use
    /// [`Command::request_with`] when this request occurrence must capture
    /// domain correlation or map the same reply type differently from another
    /// call site.
    pub fn request<P, R>(port: Port<P>, request: R) -> Self
    where
        Message: From<RequestOutcome<R::Reply>> + Send + 'static,
        P: Protocol,
        R: Request<P>,
    {
        Self::request_with(port, request, Message::from)
    }

    /// Sends a correlated request with an explicit pure continuation.
    ///
    /// Interpreting the command creates a one-shot [`ReplyTo<R::Reply>`] and
    /// converts the request through [`Request::into_message`]. A successful
    /// reply invokes the request continuation at most once with
    /// [`RequestOutcome::Replied`]. The continuation is synchronous, pure
    /// application logic and may capture a domain correlation key. The
    /// Component never waits for the reply; the mapped message arrives through
    /// its ordinary transition path. An unanswered Phase 5 Request remains a
    /// runtime-owned obligation until controlled cancellation.
    ///
    /// This candidate intentionally does not yet choose a default deadline or
    /// cancellation policy for requests.
    pub fn request_with<P, R, Map>(port: Port<P>, request: R, map: Map) -> Self
    where
        Message: Send + 'static,
        P: Protocol,
        R: Request<P>,
        Map: FnOnce(RequestOutcome<R::Reply>) -> Message + Send + 'static,
    {
        Self(CommandKind::Request(Box::new(RequestCommand {
            port,
            request,
            map,
        })))
    }

    /// Emits a typed reply through a [`RequestInvocation`]'s opaque token.
    ///
    /// Consuming the token prevents a provider transition from intentionally
    /// issuing two replies from the same authority. This command is inert data;
    /// it does not touch a channel, wake a requester, or mutate runtime state
    /// until interpreted after the provider transition commits.
    pub fn reply<ReplyValue>(reply_to: ReplyTo<ReplyValue>, reply: ReplyValue) -> Self
    where
        ReplyValue: Send + 'static,
    {
        Self(CommandKind::Reply(Box::new(Reply { reply_to, reply })))
    }

    /// Requests delivery of `message` after a runtime-controlled duration.
    ///
    /// Live execution uses real time. Controlled execution uses logical time, so
    /// tests advance it without wall-clock sleeping.
    pub fn after(delay: Duration, message: Message) -> Self {
        Self(CommandKind::After { delay, message })
    }

    /// Groups commands emitted by one transition.
    ///
    /// Grouping does not promise effect completion order. If application logic
    /// requires sequencing, that dependency needs an explicit command contract
    /// or a later message transition.
    pub fn batch(commands: impl IntoIterator<Item = Self>) -> Self {
        Self(CommandKind::Batch(commands.into_iter().collect()))
    }

    /// Returns whether this value requests no work.
    pub fn is_none(&self) -> bool {
        matches!(&self.0, CommandKind::None)
    }

    /// Finds the first concrete effect intent of type `E`, including inside a
    /// batch.
    ///
    /// This inspection hook is intended for direct transition tests; it does not
    /// execute the effect or compare mapper identity.
    pub fn effect_intent<E: EffectDescriptor>(&self) -> Option<&E> {
        self.effect_intents::<E>().into_iter().next()
    }

    /// Collects every concrete effect intent of type `E`, including inside a
    /// batch and preserving declaration traversal order.
    ///
    /// Equal-looking descriptors remain separate entries because each Command
    /// occurrence represents a distinct effect invocation. The returned order
    /// is an inspection property of this inert value; it does not promise
    /// effect completion order.
    pub fn effect_intents<E: EffectDescriptor>(&self) -> Vec<&E> {
        let mut intents = Vec::new();
        self.collect_effect_intents(&mut intents);
        intents
    }

    fn collect_effect_intents<'a, E: EffectDescriptor>(&'a self, intents: &mut Vec<&'a E>) {
        match &self.0 {
            CommandKind::Effect(command) => {
                if let Some(intent) = command.intent().downcast_ref() {
                    intents.push(intent);
                }
            }
            CommandKind::Batch(commands) => {
                for command in commands {
                    command.collect_effect_intents(intents);
                }
            }
            CommandKind::None
            | CommandKind::Send(_)
            | CommandKind::Notify(_)
            | CommandKind::Request(_)
            | CommandKind::Reply(_)
            | CommandKind::After { .. } => {}
        }
    }

    /// Applies the stored one-shot mapper when this is a top-level effect
    /// Command with concrete descriptor type `E`.
    ///
    /// This is a pure inspection and conformance hook: it supplies typed data
    /// directly and never invokes a Driver or either execution profile. The
    /// Command is consumed so its `FnOnce` mapper cannot be called twice. A
    /// non-effect Command, mismatched descriptor type, or effect that explicitly
    /// discards its outcome is returned unchanged.
    pub fn map_effect_outcome<E>(
        self,
        outcome: EffectOutcome<E::Output, E::Error>,
    ) -> Result<Message, Self>
    where
        Message: Send + 'static,
        E: EffectDescriptor,
    {
        match self.into_effect::<E>() {
            Ok(invocation) => Ok(invocation.map_outcome(outcome)),
            Err(command) => Err(command),
        }
    }

    /// Intercepts a top-level typed effect occurrence without executing it.
    ///
    /// The returned [`EffectInvocation`] owns both the non-`Clone` descriptor
    /// and its one-shot mapper. A non-effect Command or mismatched descriptor
    /// type is returned unchanged. Use [`Command::into_declarations`] first to
    /// inspect or intercept effect occurrences nested in a batch.
    pub fn into_effect<E>(self) -> Result<EffectInvocation<E, Message>, Self>
    where
        Message: Send + 'static,
        E: EffectDescriptor,
    {
        match self.0 {
            CommandKind::Effect(command)
                if command.intent().is::<E>() && command.maps_outcome() =>
            {
                let (descriptor, mapper) = command.into_parts();
                let descriptor = match descriptor.downcast::<E>() {
                    Ok(descriptor) => *descriptor,
                    Err(_) => unreachable!("the descriptor type was checked before interception"),
                };
                let mapper = mapper.expect("mapped effects retain one outcome mapper");
                let mapper = Box::new(move |outcome| mapper(Box::new(outcome)));
                Ok(EffectInvocation { descriptor, mapper })
            }
            command => Err(Self(command)),
        }
    }

    /// Collects notification intents of concrete type `N`, including inside a
    /// batch, paired with the exact named Port each notification targets.
    ///
    /// This is a direct-transition testing hook. It neither converts the value
    /// to [`Protocol::Message`] nor delivers it to a provider.
    pub fn notification_intents<N: 'static>(&self) -> Vec<(&PortId, &N)> {
        let mut intents = Vec::new();
        self.collect_notification_intents(&mut intents);
        intents
    }

    fn collect_notification_intents<'a, N: 'static>(
        &'a self,
        intents: &mut Vec<(&'a PortId, &'a N)>,
    ) {
        match &self.0 {
            CommandKind::Notify(command) => {
                if let Some(notification) = command.notification().downcast_ref() {
                    intents.push((command.port(), notification));
                }
            }
            CommandKind::Batch(commands) => {
                for command in commands {
                    command.collect_notification_intents(intents);
                }
            }
            CommandKind::None
            | CommandKind::Effect(_)
            | CommandKind::Send(_)
            | CommandKind::Request(_)
            | CommandKind::Reply(_)
            | CommandKind::After { .. } => {}
        }
    }

    /// Collects request intents of concrete type `R`, including inside a
    /// batch, paired with the exact named Port each request targets.
    ///
    /// Mapper identity and runtime correlation tokens are intentionally absent:
    /// tests assert request intent and test pure mapping functions separately.
    pub fn request_intents<R: 'static>(&self) -> Vec<(&PortId, &R)> {
        let mut intents = Vec::new();
        self.collect_request_intents(&mut intents);
        intents
    }

    fn collect_request_intents<'a, R: 'static>(&'a self, intents: &mut Vec<(&'a PortId, &'a R)>) {
        match &self.0 {
            CommandKind::Request(command) => {
                if let Some(request) = command.request().downcast_ref() {
                    intents.push((command.port(), request));
                }
            }
            CommandKind::Batch(commands) => {
                for command in commands {
                    command.collect_request_intents(intents);
                }
            }
            CommandKind::None
            | CommandKind::Effect(_)
            | CommandKind::Send(_)
            | CommandKind::Notify(_)
            | CommandKind::Reply(_)
            | CommandKind::After { .. } => {}
        }
    }

    /// Collects typed reply values, including inside a batch.
    ///
    /// The opaque correlation token is not exposed. This test hook verifies the
    /// provider's semantic reply value without making token identity part of
    /// application behavior.
    pub fn reply_intents<ReplyValue: 'static>(&self) -> Vec<&ReplyValue> {
        let mut intents = Vec::new();
        self.collect_reply_intents(&mut intents);
        intents
    }

    fn collect_reply_intents<'a, ReplyValue: 'static>(&'a self, intents: &mut Vec<&'a ReplyValue>) {
        match &self.0 {
            CommandKind::Reply(command) => {
                if let Some(reply) = command.reply().downcast_ref() {
                    intents.push(reply);
                }
            }
            CommandKind::Batch(commands) => {
                for command in commands {
                    command.collect_reply_intents(intents);
                }
            }
            CommandKind::None
            | CommandKind::Effect(_)
            | CommandKind::Send(_)
            | CommandKind::Notify(_)
            | CommandKind::Request(_)
            | CommandKind::After { .. } => {}
        }
    }

    /// Returns the concrete type name for a top-level effect command.
    ///
    /// This is diagnostic metadata, not a stable serialization identifier. A
    /// batch returns `None`; use typed inspection for behavioral tests.
    pub fn effect_type_name(&self) -> Option<&'static str> {
        match &self.0 {
            CommandKind::Effect(command) => Some(command.intent_type_name()),
            CommandKind::None
            | CommandKind::Send(_)
            | CommandKind::Notify(_)
            | CommandKind::Request(_)
            | CommandKind::Reply(_)
            | CommandKind::After { .. }
            | CommandKind::Batch(_) => None,
        }
    }

    /// Consumes a composable Command into its non-batch declarations.
    ///
    /// Direct tests and runtime interpretation use this to handle one declared
    /// operation at a time. Traversal order preserves how declarations were
    /// nested for inspectability, but does not promise execution or completion
    /// order for independent work.
    pub fn into_declarations(self) -> Vec<Self> {
        fn append<Message>(command: Command<Message>, declarations: &mut Vec<Command<Message>>) {
            match command.0 {
                CommandKind::None => {}
                CommandKind::Batch(commands) => {
                    for command in commands {
                        append(command, declarations);
                    }
                }
                declaration => declarations.push(Command(declaration)),
            }
        }

        let mut declarations = Vec::new();
        append(self, &mut declarations);
        declarations
    }
}

trait ErasedSubscription<Message>: Send {
    fn descriptor(&self) -> &dyn Any;
    fn descriptor_snapshot(&self) -> Box<dyn ErasedSourceDescriptor>;
    fn source_plan(&self) -> SourcePlan;
    fn source_event_type_id(&self) -> TypeId;
    fn descriptor_type_name(&self) -> &'static str;
    fn mapper_type_name(&self) -> &'static str;
    fn map_event(&self, event: Box<dyn Any + Send>) -> Message;
}

struct MappedSourceDescriptor<S, Map> {
    descriptor: S,
    map: Map,
}

impl<Message, S, Map> ErasedSubscription<Message> for MappedSourceDescriptor<S, Map>
where
    Message: Send + 'static,
    S: SourceDescriptor,
    Map: Fn(SourceEvent<S::Item, S::Error>) -> Message + Send + Sync + 'static,
{
    fn descriptor(&self) -> &dyn Any {
        &self.descriptor
    }

    fn descriptor_snapshot(&self) -> Box<dyn ErasedSourceDescriptor> {
        Box::new(SourceDescriptorSnapshot(self.descriptor.clone()))
    }

    fn source_plan(&self) -> SourcePlan {
        self.descriptor
            .__samara_source_plan(source_plan_private::LOWER_TOKEN)
    }

    fn source_event_type_id(&self) -> TypeId {
        TypeId::of::<SourceEvent<S::Item, S::Error>>()
    }

    fn descriptor_type_name(&self) -> &'static str {
        std::any::type_name::<S>()
    }

    fn mapper_type_name(&self) -> &'static str {
        std::any::type_name::<Map>()
    }

    fn map_event(&self, event: Box<dyn Any + Send>) -> Message {
        let event = match event.downcast::<SourceEvent<S::Item, S::Error>>() {
            Ok(event) => *event,
            Err(_) => unreachable!("source type and event type are coupled by SourceDescriptor"),
        };

        (self.map)(event)
    }
}

trait ErasedSourceDescriptor: Send {
    fn equals(&self, other: &dyn Any) -> bool;
}

struct SourceDescriptorSnapshot<S: SourceDescriptor>(S);

impl<S: SourceDescriptor> ErasedSourceDescriptor for SourceDescriptorSnapshot<S> {
    fn equals(&self, other: &dyn Any) -> bool {
        other.downcast_ref::<S>() == Some(&self.0)
    }
}

/// One declarative request for an ongoing source of messages.
///
/// A subscription's logical identity is its owning [`ComponentId`] plus
/// [`SubscriptionId`]. The [`SourceDescriptor`] value is comparable
/// configuration: unchanged configuration remains active, while changed
/// configuration is replaced or reconfigured. The message mapper converts
/// repeated [`SourceEvent`] values to Component messages and is deliberately
/// excluded from reconciliation identity.
pub struct Subscription<Message> {
    id: SubscriptionId,
    descriptor: Box<dyn ErasedSubscription<Message>>,
}

impl<Message> Subscription<Message> {
    /// Declares a typed source using its canonical Message conversion.
    ///
    /// This short form uses `Message: From<SourceEvent<...>>`. Use
    /// [`Subscription::source_with`] when this Subscription identity must
    /// capture domain context or map the same event type differently from
    /// another Source.
    pub fn source<S>(id: SubscriptionId, descriptor: S) -> Self
    where
        Message: From<SourceEvent<S::Item, S::Error>> + Send + 'static,
        S: SourceDescriptor,
    {
        Self::source_with(id, descriptor, Message::from)
    }

    /// Declares a typed source with an explicit pure event-to-message mapping.
    ///
    /// Constructing this value starts no task and touches no external resource.
    /// `Map` is called repeatedly for the lifetime of an active source, so it is
    /// `Fn` rather than the one-shot `FnOnce` accepted by
    /// [`Command::effect_with`].
    pub fn source_with<S, Map>(id: SubscriptionId, descriptor: S, map: Map) -> Self
    where
        Message: Send + 'static,
        S: SourceDescriptor,
        Map: Fn(SourceEvent<S::Item, S::Error>) -> Message + Send + Sync + 'static,
    {
        Self {
            id,
            descriptor: Box::new(MappedSourceDescriptor { descriptor, map }),
        }
    }

    /// Returns the stable key used to reconcile this subscription.
    pub fn id(&self) -> &SubscriptionId {
        &self.id
    }

    /// Returns the concrete source descriptor when it has type `S`.
    ///
    /// Direct Component tests use this to assert the desired SourceDescriptor
    /// without starting a live Driver.
    pub fn source_descriptor<S: SourceDescriptor>(&self) -> Option<&S> {
        self.descriptor.descriptor().downcast_ref()
    }

    /// Applies this subscription's reusable mapper to one typed Source event.
    ///
    /// This pure inspection and conformance hook does not create a Source,
    /// select an execution profile, or deliver the resulting Message. The
    /// mapper is borrowed and can therefore be applied to every event from one
    /// active Source. A mismatched descriptor type returns the event unchanged.
    pub fn map_source_event<S>(
        &self,
        event: SourceEvent<S::Item, S::Error>,
    ) -> Result<Message, SourceEvent<S::Item, S::Error>>
    where
        S: SourceDescriptor,
    {
        if self.descriptor.descriptor().is::<S>() {
            Ok(self.descriptor.map_event(Box::new(event)))
        } else {
            Err(event)
        }
    }

    /// Returns diagnostic Rust type metadata for the source descriptor.
    ///
    /// The returned name is not a stable protocol or serialization identifier.
    pub fn descriptor_type_name(&self) -> &'static str {
        self.descriptor.descriptor_type_name()
    }

    fn descriptor_snapshot(&self) -> Box<dyn ErasedSourceDescriptor> {
        self.descriptor.descriptor_snapshot()
    }

    pub(crate) fn source_plan(&self) -> SourcePlan {
        self.descriptor.source_plan()
    }

    pub(crate) fn source_event_type_id(&self) -> TypeId {
        self.descriptor.source_event_type_id()
    }

    pub(crate) fn descriptor_any(&self) -> &dyn Any {
        self.descriptor.descriptor()
    }

    pub(crate) fn map_erased_source_event(&self, event: ErasedSourceEvent) -> Message {
        self.descriptor.map_event(event.value)
    }

    fn has_descriptor(&self, descriptor: &dyn ErasedSourceDescriptor) -> bool {
        descriptor.equals(self.descriptor.descriptor())
    }
}

/// Complete desired subscription set for one Component model.
///
/// The runtime compares this set with the active set after a committed
/// transition. Omitting a previously desired identity requests cancellation.
pub struct Subscriptions<Message>(Vec<Subscription<Message>>);

impl<Message> Subscriptions<Message> {
    /// Declares no ongoing sources.
    pub fn none() -> Self {
        Self(Vec::new())
    }

    /// Creates a desired set containing exactly one subscription.
    pub fn one(subscription: Subscription<Message>) -> Self {
        Self(vec![subscription])
    }

    /// Iterates over the desired subscriptions without executing them.
    pub fn iter(&self) -> impl Iterator<Item = &Subscription<Message>> {
        self.0.iter()
    }

    /// Finds SourceDescriptor `S` for one stable subscription identity.
    ///
    /// This is primarily an inspection helper for direct Component tests.
    pub fn find<S: SourceDescriptor>(&self, id: &SubscriptionId) -> Option<&S> {
        self.0
            .iter()
            .find(|subscription| subscription.id() == id)
            .and_then(Subscription::source_descriptor)
    }
}

impl<Message> From<Vec<Subscription<Message>>> for Subscriptions<Message> {
    fn from(value: Vec<Subscription<Message>>) -> Self {
        Self(value)
    }
}

/// Inert description of a named logical stream of `T` values.
///
/// This descriptor is safe to store in a Component and does not prescribe how
/// the stream is realized. Live assembly may bind a concrete Tokio
/// `mpsc::Receiver<T>` through [`LiveRuntimeBuilder::bind_mpsc`], while
/// controlled tests script the same logical stream through
/// [`ControlledRuntimeBuilder::control_stream`]. Normal stream closure is
/// delivered as [`SourceEvent::Ended`].
pub struct StreamDescriptor<T> {
    binding: Arc<str>,
    marker: PhantomData<fn() -> T>,
}

impl<T> StreamDescriptor<T> {
    /// Creates a reusable logical stream name.
    pub fn named(binding: impl Into<Arc<str>>) -> Self {
        Self {
            binding: binding.into(),
            marker: PhantomData,
        }
    }
}

impl<T> Clone for StreamDescriptor<T> {
    fn clone(&self) -> Self {
        Self {
            binding: self.binding.clone(),
            marker: PhantomData,
        }
    }
}

impl<T> PartialEq for StreamDescriptor<T> {
    fn eq(&self, other: &Self) -> bool {
        self.binding == other.binding
    }
}

impl<T> Eq for StreamDescriptor<T> {}

impl<T> fmt::Debug for StreamDescriptor<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("StreamDescriptor")
            .field(&self.binding)
            .finish()
    }
}

impl<T: Send + 'static> SourceDescriptor for StreamDescriptor<T> {
    type Item = T;
    type Error = std::convert::Infallible;
}

/// Inert description of one finite HTTP request.
///
/// The descriptor owns its method, URL text, headers, and body. Constructing it
/// performs no parsing, DNS lookup, socket access, or other I/O. Live execution
/// realizes it through [`LiveRuntimeBuilder::bind_http`], while controlled
/// execution intercepts it through
/// [`ControlledRuntimeBuilder::control_effect`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpRequest {
    method: http::Method,
    url: Arc<str>,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl HttpRequest {
    /// Describes a request with an empty header map and body.
    ///
    /// URL validation is deliberately deferred to the terminal Driver so
    /// invalid or unsupported live configuration becomes a typed [`HttpError`].
    pub fn new(method: http::Method, url: impl Into<Arc<str>>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: http::HeaderMap::new(),
            body: bytes::Bytes::new(),
        }
    }

    /// Describes a `GET` request with an empty header map and body.
    pub fn get(url: impl Into<Arc<str>>) -> Self {
        Self::new(http::Method::GET, url)
    }

    /// Returns the declared HTTP method.
    pub fn method(&self) -> &http::Method {
        &self.method
    }

    /// Returns the exact URL text supplied by the application.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Returns the declared request headers.
    pub fn headers(&self) -> &http::HeaderMap {
        &self.headers
    }

    /// Returns mutable access to the declared request headers.
    ///
    /// `HeaderMap::append` can be used when repeated field values must be
    /// preserved.
    pub fn headers_mut(&mut self) -> &mut http::HeaderMap {
        &mut self.headers
    }

    /// Appends one request header and returns the updated descriptor.
    ///
    /// Appending, rather than replacing, preserves repeated field values.
    pub fn with_header(mut self, name: http::HeaderName, value: http::HeaderValue) -> Self {
        self.headers.append(name, value);
        self
    }

    /// Returns the complete owned request body.
    pub fn body(&self) -> &bytes::Bytes {
        &self.body
    }

    /// Replaces the request body.
    pub fn with_body(mut self, body: impl Into<bytes::Bytes>) -> Self {
        self.body = body.into();
        self
    }

    /// Crosses from request construction into pure response handling.
    ///
    /// This consumes the request and returns a distinct, inert type. Request
    /// modifiers are deliberately unavailable after the boundary:
    ///
    /// ```compile_fail
    /// use samara::HttpRequest;
    ///
    /// HttpRequest::get("https://example.test")
    ///     .on_response()
    ///     .with_body("too late");
    /// ```
    pub fn on_response(self) -> HttpResponsePipeline<HttpResponse, HttpError> {
        HttpResponsePipeline {
            request: self,
            transform: Box::new(|outcome| outcome),
        }
    }

    pub(crate) fn into_parts(self) -> (http::Method, Arc<str>, http::HeaderMap, bytes::Bytes) {
        (self.method, self.url, self.headers, self.body)
    }
}

impl EffectDescriptor for HttpRequest {
    type Output = HttpResponse;
    type Error = HttpError;
}

/// Complete raw output of one first-party [`HttpRequest`] effect.
///
/// Every HTTP status is a successful response value. The terminal Driver does
/// not interpret redirects, 4xx or 5xx statuses, content types, or body bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    status: http::StatusCode,
    version: http::Version,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl HttpResponse {
    /// Constructs a complete raw response, including for controlled fixtures.
    pub fn new(
        status: http::StatusCode,
        version: http::Version,
        headers: http::HeaderMap,
        body: impl Into<bytes::Bytes>,
    ) -> Self {
        Self {
            status,
            version,
            headers,
            body: body.into(),
        }
    }

    /// Returns the raw HTTP status without applying success policy.
    pub fn status(&self) -> http::StatusCode {
        self.status
    }

    /// Returns the HTTP protocol version reported by the server.
    pub fn version(&self) -> http::Version {
        self.version
    }

    /// Returns all response headers.
    pub fn headers(&self) -> &http::HeaderMap {
        &self.headers
    }

    /// Returns the fully buffered response body.
    pub fn body(&self) -> &bytes::Bytes {
        &self.body
    }

    /// Consumes the response and returns its fully buffered body.
    pub fn into_body(self) -> bytes::Bytes {
        self.body
    }
}

/// Stage at which a first-party HTTP effect failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpErrorKind {
    /// The client or request could not be configured from the descriptor.
    Configuration,
    /// Sending the request or receiving its complete response failed.
    Transport,
}

/// Typed explanatory data for a first-party HTTP effect failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpError {
    kind: HttpErrorKind,
    message: Arc<str>,
}

impl HttpError {
    /// Creates a typed HTTP failure for controlled execution and fixtures.
    ///
    /// Live execution constructs this value from its HTTP client diagnostics.
    pub fn new(kind: HttpErrorKind, message: impl Into<Arc<str>>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub(crate) fn configuration(error: impl fmt::Display) -> Self {
        Self::new(HttpErrorKind::Configuration, error.to_string())
    }

    pub(crate) fn transport(error: impl fmt::Display) -> Self {
        Self::new(HttpErrorKind::Transport, error.to_string())
    }

    /// Returns whether configuration or transport failed.
    pub fn kind(&self) -> HttpErrorKind {
        self.kind
    }

    /// Returns the underlying client diagnostic as explanatory text.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for HttpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "HTTP {:?} error: {}", self.kind, self.message)
    }
}

impl Error for HttpError {}

type HttpResponseTransform<Output, ResponseError> = Box<
    dyn FnOnce(EffectOutcome<HttpResponse, HttpError>) -> EffectOutcome<Output, ResponseError>
        + Send
        + 'static,
>;

/// Inert, ordered pure handling for one owned [`HttpRequest`] response.
///
/// This value performs no I/O and is not a separately bound or traced effect.
/// [`HttpResponsePipeline::into_command`] or
/// [`HttpResponsePipeline::into_command_with`] lowers it to the original raw
/// request plus one composed, one-shot message mapper. It intentionally does
/// not implement `Clone`: the request and every declared transform are owned
/// and may be consumed only once.
///
/// ```compile_fail
/// use samara::HttpRequest;
///
/// let pipeline = HttpRequest::get("https://example.test").on_response();
/// let duplicate = pipeline.clone();
/// # drop(duplicate);
/// ```
#[must_use = "an HTTP response pipeline is inert until converted into a Command"]
pub struct HttpResponsePipeline<Output, ResponseError> {
    request: HttpRequest,
    transform: HttpResponseTransform<Output, ResponseError>,
}

impl<Output, ResponseError> HttpResponsePipeline<Output, ResponseError>
where
    Output: Send + 'static,
    ResponseError: Send + 'static,
{
    fn then<NextOutput, NextError, Transform>(
        self,
        transform_next: Transform,
    ) -> HttpResponsePipeline<NextOutput, NextError>
    where
        NextOutput: Send + 'static,
        NextError: Send + 'static,
        Transform: FnOnce(EffectOutcome<Output, ResponseError>) -> EffectOutcome<NextOutput, NextError>
            + Send
            + 'static,
    {
        let Self { request, transform } = self;
        HttpResponsePipeline {
            request,
            transform: Box::new(move |outcome| transform_next(transform(outcome))),
        }
    }

    /// Lowers this pipeline using its canonical Message conversion.
    ///
    /// This short form uses `Message: From<EffectOutcome<Output,
    /// ResponseError>>`. Use [`HttpResponsePipeline::into_command_with`] when
    /// this endpoint occurrence needs captured context or a distinct Message
    /// projection.
    pub fn into_command<Message>(self) -> Command<Message>
    where
        Message: From<EffectOutcome<Output, ResponseError>> + Send + 'static,
    {
        self.into_command_with(Message::from)
    }

    /// Lowers this pure pipeline with an explicit Message mapper.
    ///
    /// Live execution still selects the [`HttpRequest`] Driver and controlled
    /// execution still intercepts `next_effect::<HttpRequest>()`. The response
    /// transforms and `map` run synchronously and at most once after that raw
    /// terminal outcome is accepted.
    pub fn into_command_with<Message, Map>(self, map: Map) -> Command<Message>
    where
        Message: Send + 'static,
        Map: FnOnce(EffectOutcome<Output, ResponseError>) -> Message + Send + 'static,
    {
        let Self { request, transform } = self;
        Command::effect_with(request, move |outcome| map(transform(outcome)))
    }
}

impl HttpResponsePipeline<HttpResponse, HttpError> {
    /// Requires the response status to be in the inclusive 200–299 range.
    ///
    /// Any other status becomes [`HttpResponseError::Status`] and retains the
    /// complete response. Later transforms do not run for a rejected response.
    /// Apply this before body decoding; after `json::<T>()`, the pipeline no
    /// longer carries a raw response:
    ///
    /// ```compile_fail
    /// use samara::HttpRequest;
    /// use serde::Deserialize;
    ///
    /// #[derive(Deserialize)]
    /// struct Reply { value: u64 }
    ///
    /// HttpRequest::get("https://example.test")
    ///     .on_response()
    ///     .json::<Reply>()
    ///     .require_success();
    /// ```
    pub fn require_success(self) -> HttpResponsePipeline<HttpResponse, HttpResponseError> {
        self.then(|outcome| match outcome {
            EffectOutcome::Succeeded(response) if response.status().is_success() => {
                EffectOutcome::Succeeded(response)
            }
            EffectOutcome::Succeeded(response) => {
                EffectOutcome::Failed(HttpStatusError::new(response).into())
            }
            EffectOutcome::Failed(error) => EffectOutcome::Failed(error.into()),
            EffectOutcome::Cancelled(reason) => EffectOutcome::Cancelled(reason),
        })
    }
}

impl<ResponseError> HttpResponsePipeline<HttpResponse, ResponseError>
where
    ResponseError: Into<HttpResponseError> + Send + 'static,
{
    /// Decodes the complete owned response body as JSON.
    ///
    /// This operation adds no status policy. In particular, a valid JSON body
    /// on a 4xx or 5xx response succeeds unless [`Self::require_success`] was
    /// explicitly applied first. A decoding failure retains both the complete
    /// raw response and the original [`serde_json::Error`].
    pub fn json<T>(self) -> HttpResponsePipeline<T, HttpResponseError>
    where
        T: serde::de::DeserializeOwned + Send + 'static,
    {
        self.then(|outcome| match outcome {
            EffectOutcome::Succeeded(response) => {
                let decoded = serde_json::from_slice(response.body());
                match decoded {
                    Ok(output) => EffectOutcome::Succeeded(output),
                    Err(source) => {
                        EffectOutcome::Failed(HttpJsonError::new(source, response).into())
                    }
                }
            }
            EffectOutcome::Failed(error) => EffectOutcome::Failed(error.into()),
            EffectOutcome::Cancelled(reason) => EffectOutcome::Cancelled(reason),
        })
    }
}

/// Typed response-pipeline failures after a raw HTTP terminal outcome.
///
/// Runtime cancellation is deliberately absent: it remains the separate
/// [`EffectOutcome::Cancelled`] terminal condition.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HttpResponseError {
    /// The raw request could not be configured or transported.
    #[error(transparent)]
    Http(#[from] HttpError),
    /// An explicit success-status policy rejected the complete response.
    #[error(transparent)]
    Status(#[from] HttpStatusError),
    /// The complete response body could not be decoded as JSON.
    #[error(transparent)]
    Json(#[from] HttpJsonError),
}

impl HttpResponseError {
    /// Returns the raw HTTP error when configuration or transport failed.
    pub fn http_error(&self) -> Option<&HttpError> {
        match self {
            Self::Http(error) => Some(error),
            Self::Status(_) | Self::Json(_) => None,
        }
    }

    /// Returns the retained response for status-policy or JSON failures.
    pub fn response(&self) -> Option<&HttpResponse> {
        match self {
            Self::Http(_) => None,
            Self::Status(error) => Some(error.response()),
            Self::Json(error) => Some(error.response()),
        }
    }
}

/// A complete non-2xx response rejected by [`HttpResponsePipeline::require_success`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpStatusError {
    response: HttpResponse,
}

impl HttpStatusError {
    fn new(response: HttpResponse) -> Self {
        Self { response }
    }

    /// Returns the rejected status code.
    pub fn status(&self) -> http::StatusCode {
        self.response.status()
    }

    /// Borrows the complete rejected response.
    pub fn response(&self) -> &HttpResponse {
        &self.response
    }

    /// Consumes the error and returns the complete rejected response.
    pub fn into_response(self) -> HttpResponse {
        self.response
    }
}

impl fmt::Display for HttpStatusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "HTTP response status {} is outside the required 2xx range",
            self.status()
        )
    }
}

impl Error for HttpStatusError {}

/// JSON decoding failure retaining its source and complete raw response.
#[derive(Debug, thiserror::Error)]
#[error("failed to decode HTTP response body as JSON: {source}")]
pub struct HttpJsonError {
    source: serde_json::Error,
    response: HttpResponse,
}

impl HttpJsonError {
    fn new(source: serde_json::Error, response: HttpResponse) -> Self {
        Self { source, response }
    }

    /// Returns the original JSON decoder error.
    pub fn source(&self) -> &serde_json::Error {
        &self.source
    }

    /// Borrows the complete response whose body failed to decode.
    pub fn response(&self) -> &HttpResponse {
        &self.response
    }

    /// Consumes the error and returns the complete response.
    pub fn into_response(self) -> HttpResponse {
        self.response
    }

    /// Consumes the error and returns both decoder source and raw response.
    pub fn into_parts(self) -> (serde_json::Error, HttpResponse) {
        (self.source, self.response)
    }
}

/// Inert description of one finite best-effort print to process standard output.
///
/// Constructing this value performs no I/O. Live execution realizes it through
/// [`LiveRuntimeBuilder::bind_stdio`], while controlled execution can intercept
/// it through [`ControlledRuntimeBuilder::control_effect`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrintStdout {
    text: String,
}

impl PrintStdout {
    /// Describes an exact UTF-8 text write with no implicit terminator.
    pub fn text(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    /// Describes a UTF-8 text write followed by one `\n` byte.
    ///
    /// The newline is unconditional; text already ending in `\n` therefore
    /// produces one additional blank line, matching `println!` behavior.
    pub fn line(text: impl Into<String>) -> Self {
        let mut text = text.into();
        text.push('\n');
        Self::text(text)
    }

    /// Returns the exact text this effect asks the Driver to print.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub(crate) fn into_text(self) -> String {
        self.text
    }
}

impl EffectDescriptor for PrintStdout {
    type Output = ();
    type Error = std::convert::Infallible;
}

/// Inert description of one finite best-effort print to process standard error.
///
/// Constructing this value performs no I/O. Live execution realizes it through
/// [`LiveRuntimeBuilder::bind_stdio`], while controlled execution can intercept
/// it through [`ControlledRuntimeBuilder::control_effect`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrintStderr {
    text: String,
}

impl PrintStderr {
    /// Describes an exact UTF-8 text write with no implicit terminator.
    pub fn text(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    /// Describes a UTF-8 text write followed by one `\n` byte.
    ///
    /// The newline is unconditional; text already ending in `\n` therefore
    /// produces one additional blank line, matching `eprintln!` behavior.
    pub fn line(text: impl Into<String>) -> Self {
        let mut text = text.into();
        text.push('\n');
        Self::text(text)
    }

    /// Returns the exact text this effect asks the Driver to print.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub(crate) fn into_text(self) -> String {
        self.text
    }
}

impl EffectDescriptor for PrintStderr {
    type Output = ();
    type Error = std::convert::Infallible;
}

/// Inert description of one TCP byte-stream connection.
///
/// Each live Source realization makes exactly one connection attempt to the
/// supplied numeric socket address. DNS, reconnect, retry, framing, TLS, and
/// domain interpretation are deliberately outside this terminal descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TcpBytes {
    endpoint: SocketAddr,
}

impl TcpBytes {
    /// Describes one connection to `endpoint`.
    pub fn connect(endpoint: SocketAddr) -> Self {
        Self { endpoint }
    }

    /// Returns the numeric endpoint used for the one connection attempt.
    pub fn endpoint(&self) -> SocketAddr {
        self.endpoint
    }
}

impl SourceDescriptor for TcpBytes {
    type Item = bytes::Bytes;
    type Error = TcpError;
}

/// Stage at which the first-party TCP Source failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TcpErrorKind {
    /// Establishing the single connection failed.
    Connect,
    /// Reading the established byte stream failed.
    Read,
}

/// Typed explanatory data for a first-party TCP Source failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TcpError {
    kind: TcpErrorKind,
    message: Arc<str>,
}

impl TcpError {
    /// Creates a typed TCP failure for controlled execution and test fixtures.
    ///
    /// Live execution constructs this value from Tokio I/O errors. Controlled
    /// execution has no live error to wrap, so tests use this constructor to
    /// supply the same explanatory boundary value explicitly.
    pub fn new(kind: TcpErrorKind, message: impl Into<Arc<str>>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    fn connect(error: std::io::Error) -> Self {
        Self::new(TcpErrorKind::Connect, error.to_string())
    }

    fn read(error: std::io::Error) -> Self {
        Self::new(TcpErrorKind::Read, error.to_string())
    }

    /// Returns whether connection or established-stream reading failed.
    pub fn kind(&self) -> TcpErrorKind {
        self.kind
    }

    /// Returns the operating-system diagnostic without exposing it as policy.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for TcpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "TCP {:?} error: {}", self.kind, self.message)
    }
}

impl Error for TcpError {}

/// Pure streaming decoder used by [`Framed`].
///
/// The decoder value is comparable configuration. Mutable operational state,
/// such as an incomplete-frame buffer, is created with [`Decoder::start`] and
/// owned by the runtime for one active subscription instance.
pub trait Decoder: Clone + PartialEq + fmt::Debug + Send + Sync + 'static {
    /// Raw chunk accepted from the underlying source.
    type Chunk: Send + 'static;
    /// Decoded application value emitted by the framed source.
    type Frame: Send + 'static;
    /// Typed error explaining a decoding failure.
    type Error: Send + 'static;
    /// Per-subscription mutable decoder state.
    type State: Send + 'static;

    /// Creates fresh operational state when a framed subscription starts.
    fn start(&self) -> Self::State;

    /// Purely incorporates one chunk and emits zero or more complete frames.
    fn push(
        &self,
        state: &mut Self::State,
        chunk: Self::Chunk,
    ) -> Result<Vec<Self::Frame>, Self::Error>;

    /// Purely finalizes decoder state after normal end of the byte stream.
    ///
    /// This method is called exactly once for a normally ending underlying
    /// Source. It must either return every final frame in order or reject the
    /// remaining state with a typed Error. Source failure, replacement,
    /// cancellation, shutdown, runtime fault, and an earlier decode failure do
    /// not call `finish`.
    fn finish(&self, state: &mut Self::State) -> Result<Vec<Self::Frame>, Self::Error>;
}

/// Source Description Layer that applies a pure stateful [`Decoder`] to another
/// [`SourceDescriptor`].
///
/// Live execution binds the underlying source descriptor to a world-facing
/// Driver.
/// Controlled execution injects underlying chunks, ensuring the identical
/// decoder and partial-frame behavior run in both profiles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Framed<S, D> {
    /// Underlying source of raw decoder chunks.
    pub source: S,
    /// Pure decoder configuration shared across execution profiles.
    pub decoder: D,
}

impl<S, D> Framed<S, D> {
    /// Composes an underlying source with a decoder configuration.
    pub fn new(source: S, decoder: D) -> Self {
        Self { source, decoder }
    }
}

impl<S, D> Framed<S, D>
where
    S: SourceDescriptor<Item = D::Chunk>,
    D: Decoder,
{
    /// Creates the pure runtime-scoped Layer state for one descriptor instance.
    ///
    /// This does not create a Source, invoke a Driver, or choose live versus
    /// controlled execution. Both profiles use the same Layer to transform
    /// events from the inner descriptor vocabulary.
    pub fn into_layer(self) -> FramedLayer<S, D> {
        let state = self.decoder.start();
        FramedLayer {
            descriptor: self,
            state,
        }
    }
}

/// Profile-independent state for one active [`Framed`] composition.
///
/// The runtime creates one value per active composed Source. Its decoder state
/// is deterministic mechanism state rather than Component Model state or a
/// world-facing resource. Mapping an event produces only outer [`SourceEvent`]
/// values; delivery and terminal Source behavior remain runtime concerns.
pub struct FramedLayer<S, D>
where
    S: SourceDescriptor<Item = D::Chunk>,
    D: Decoder,
{
    descriptor: Framed<S, D>,
    state: D::State,
}

type FramedLayerEvent<S, D> = SourceEvent<
    <D as Decoder>::Frame,
    FramedError<<S as SourceDescriptor>::Error, <D as Decoder>::Error>,
>;

impl<S, D> FramedLayer<S, D>
where
    S: SourceDescriptor<Item = D::Chunk>,
    D: Decoder,
{
    /// Returns the next descriptor inside this composed Layer.
    ///
    /// The returned descriptor may itself be composed; this method does not
    /// claim that it is the terminal Driver boundary.
    pub fn inner_descriptor(&self) -> &S {
        &self.descriptor.source
    }

    /// Purely transforms one inner Source event into zero or more outer events.
    ///
    /// A chunk may yield several frames or no frame while decoder state retains
    /// an incomplete suffix. Inner and decoder failures remain distinguished as
    /// typed data. Producing a terminal event does not itself cancel a Source or
    /// deliver a Component Message.
    pub fn map_event(
        &mut self,
        event: SourceEvent<S::Item, S::Error>,
    ) -> Vec<FramedLayerEvent<S, D>> {
        match event {
            SourceEvent::Item(chunk) => {
                match self.descriptor.decoder.push(&mut self.state, chunk) {
                    Ok(frames) => frames.into_iter().map(SourceEvent::Item).collect(),
                    Err(error) => vec![SourceEvent::Failed(FramedError::Decode(error))],
                }
            }
            SourceEvent::Failed(error) => {
                vec![SourceEvent::Failed(FramedError::Source(error))]
            }
            SourceEvent::Ended => match self.descriptor.decoder.finish(&mut self.state) {
                Ok(frames) => frames
                    .into_iter()
                    .map(SourceEvent::Item)
                    .chain(std::iter::once(SourceEvent::Ended))
                    .collect(),
                Err(error) => vec![SourceEvent::Failed(FramedError::Decode(error))],
            },
        }
    }
}

/// Typed error explaining failure in either part of a [`Framed`] Layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FramedError<SourceError, DecodeError> {
    /// The underlying world-facing source failed.
    Source(SourceError),
    /// The pure decoder rejected input from an otherwise active source.
    Decode(DecodeError),
}

impl<S, D> SourceDescriptor for Framed<S, D>
where
    S: SourceDescriptor<Item = D::Chunk>,
    D: Decoder,
{
    type Item = D::Frame;
    type Error = FramedError<S::Error, D::Error>;

    #[allow(private_interfaces)]
    fn __samara_source_plan(&self, _: source_plan_private::LowerToken) -> SourcePlan {
        let mut plan = self
            .source
            .__samara_source_plan(source_plan_private::LOWER_TOKEN);
        plan.valid_event_chain &=
            plan.output_event_type == TypeId::of::<SourceEvent<S::Item, S::Error>>();
        plan.layers.push(Box::new(self.clone().into_layer()));
        plan.output_event_type =
            TypeId::of::<SourceEvent<D::Frame, FramedError<S::Error, D::Error>>>();
        plan
    }
}

impl<S, D> ErasedSourceLayer for FramedLayer<S, D>
where
    S: SourceDescriptor<Item = D::Chunk>,
    D: Decoder,
{
    fn map_event(&mut self, event: ErasedSourceEvent) -> Vec<ErasedSourceEvent> {
        let event = match event.value.downcast::<SourceEvent<S::Item, S::Error>>() {
            Ok(event) => *event,
            Err(_) => unreachable!("SourcePlan couples every Layer to its input event type"),
        };

        self.map_event(event)
            .into_iter()
            .map(|event| {
                let kind = SourceEventKind::of(&event);
                ErasedSourceEvent {
                    value: Box::new(event),
                    kind,
                }
            })
            .collect()
    }
}

/// Named, immutable logical dependency on a provider-neutral [`Protocol`].
///
/// A Component may store this value and use it with [`Command::notify`] or
/// [`Command::request`]. It contains no provider reference, channel, runtime handle,
/// or lookup capability. [`ProgramBuilder::bind_port`] selects the provider
/// during program assembly, allowing the same Component definition to receive
/// real, mock, or controlled providers without changing its transition logic.
pub struct Port<P: Protocol> {
    id: PortId,
    program: Arc<()>,
    marker: PhantomData<fn() -> P>,
}

impl<P: Protocol> Port<P> {
    /// Returns this dependency's stable logical name.
    pub fn id(&self) -> &PortId {
        &self.id
    }
}

impl<P: Protocol> Clone for Port<P> {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            program: self.program.clone(),
            marker: PhantomData,
        }
    }
}

impl<P: Protocol> PartialEq for Port<P> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<P: Protocol> Eq for Port<P> {}

impl<P: Protocol> fmt::Debug for Port<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Port").field(&self.id).finish()
    }
}

/// Typed logical address of a Component in a [`Program`].
///
/// A Component may retain this value and pass it to [`Command::send`] when tight
/// coupling to another Component's complete message API is intentional. Normal
/// reusable dependencies should prefer [`Port`], which exposes only a stable
/// provider-neutral protocol. A Component reference exposes no model access and
/// does not itself send messages, so transitions receive no runtime capability.
/// This address is independent of mailbox or task topology.
///
/// Delivery remains a runtime-interpreted Command rather than a capability on
/// the reference itself:
///
/// ```compile_fail
/// use samara::{Component, ComponentRef};
///
/// fn bypass_runtime<C: Component>(target: &ComponentRef<C>, message: C::Message) {
///     target.send(message);
/// }
/// ```
pub struct ComponentRef<C: Component> {
    id: ComponentId,
    program: Arc<()>,
    marker: PhantomData<fn() -> C>,
}

impl<C: Component> ComponentRef<C> {
    /// Returns the target's stable program identity.
    pub fn id(&self) -> &ComponentId {
        &self.id
    }
}

impl<C: Component> Clone for ComponentRef<C> {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            program: self.program.clone(),
            marker: PhantomData,
        }
    }
}

impl<C: Component> fmt::Debug for ComponentRef<C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ComponentRef")
            .field(&self.id)
            .finish()
    }
}

/// Topology-neutral application blueprint assembled from Components.
///
/// A program contains logical Component, effect, source, and protocol
/// relationships but no live Tokio resources. Call the same program factory for
/// [`LiveRuntime`] and [`ControlledRuntime`] assembly.
pub struct Program {
    program: Arc<()>,
    components: Vec<Box<dyn ErasedComponentKernel>>,
    bindings: Vec<Box<dyn ErasedPortBinding>>,
}

impl Program {
    /// Begins assembling a program blueprint.
    pub fn builder() -> ProgramBuilder {
        let program = Arc::new(());
        ProgramBuilder {
            program,
            components: Vec::new(),
            ports: Vec::new(),
            bindings: Vec::new(),
        }
    }
}

struct PortDeclaration {
    protocol_type: TypeId,
    protocol_type_name: &'static str,
    id: PortId,
    program: Arc<()>,
}

pub(crate) trait ErasedPortBinding: Send {
    fn protocol_type(&self) -> TypeId;
    fn protocol_type_name(&self) -> &'static str;
    fn port_id(&self) -> &PortId;
    fn port_program(&self) -> &Arc<()>;
    fn provider_id(&self) -> &ComponentId;
    fn provider_program(&self) -> &Arc<()>;
    fn provider_component_type(&self) -> TypeId;
    fn provider_message_type(&self) -> TypeId;
    fn provider_message_type_name(&self) -> &'static str;
    fn convert(&self, message: Box<dyn Any + Send>) -> Box<dyn Any + Send>;
}

struct PortBinding<P, C>
where
    P: Protocol,
    C: Component,
{
    port: Port<P>,
    provider: ComponentRef<C>,
}

impl<P, C> ErasedPortBinding for PortBinding<P, C>
where
    P: Protocol,
    C: Component,
    C::Message: From<P::Message>,
{
    fn protocol_type(&self) -> TypeId {
        TypeId::of::<P>()
    }

    fn protocol_type_name(&self) -> &'static str {
        std::any::type_name::<P>()
    }

    fn port_id(&self) -> &PortId {
        self.port.id()
    }

    fn port_program(&self) -> &Arc<()> {
        &self.port.program
    }

    fn provider_id(&self) -> &ComponentId {
        self.provider.id()
    }

    fn provider_program(&self) -> &Arc<()> {
        &self.provider.program
    }

    fn provider_component_type(&self) -> TypeId {
        TypeId::of::<C>()
    }

    fn provider_message_type(&self) -> TypeId {
        TypeId::of::<C::Message>()
    }

    fn provider_message_type_name(&self) -> &'static str {
        std::any::type_name::<C::Message>()
    }

    fn convert(&self, message: Box<dyn Any + Send>) -> Box<dyn Any + Send> {
        let message = match message.downcast::<P::Message>() {
            Ok(message) => *message,
            Err(_) => unreachable!("a Port binding couples one Protocol Message type"),
        };
        Box::new(C::Message::from(message))
    }
}

/// Explicitly knowable error in topology-neutral program assembly.
///
/// The variants deliberately cover only facts represented by
/// [`ProgramBuilder`]. Port cycles and behavior-dependent direct sends are not
/// treated as a statically closed dependency graph.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProgramBuildError {
    /// Two registered Components claimed the same logical identity.
    DuplicateComponentId(ComponentId),
    /// One Protocol and Port name pair was declared more than once.
    DuplicatePort {
        /// Diagnostic Rust Protocol type name.
        protocol_type: &'static str,
        /// Duplicated logical Port name.
        port: PortId,
    },
    /// A declared Port had zero or more than one provider binding.
    PortBindingCount {
        /// Diagnostic Rust Protocol type name.
        protocol_type: &'static str,
        /// Logical Port name.
        port: PortId,
        /// Number of bindings found.
        count: usize,
    },
    /// A binding used a Port declared by another builder.
    ForeignPort {
        /// Diagnostic Rust Protocol type name.
        protocol_type: &'static str,
        /// Logical Port name.
        port: PortId,
    },
    /// A binding selected a provider registered by another builder.
    ForeignProvider {
        /// Logical provider identity.
        provider: ComponentId,
    },
    /// A provider reference did not name a matching registered Component.
    ProviderNotRegistered {
        /// Logical provider identity.
        provider: ComponentId,
    },
}

impl fmt::Display for ProgramBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateComponentId(id) => {
                write!(formatter, "duplicate Component identity {id:?}")
            }
            Self::DuplicatePort {
                protocol_type,
                port,
            } => write!(formatter, "duplicate Port {port:?} for {protocol_type}"),
            Self::PortBindingCount {
                protocol_type,
                port,
                count,
            } => write!(
                formatter,
                "Port {port:?} for {protocol_type} has {count} bindings; expected exactly one"
            ),
            Self::ForeignPort {
                protocol_type,
                port,
            } => write!(
                formatter,
                "Port {port:?} for {protocol_type} belongs to another ProgramBuilder"
            ),
            Self::ForeignProvider { provider } => write!(
                formatter,
                "provider {provider:?} belongs to another ProgramBuilder"
            ),
            Self::ProviderNotRegistered { provider } => {
                write!(formatter, "provider {provider:?} is not registered")
            }
        }
    }
}

impl Error for ProgramBuildError {}

/// Mutable builder used to register Components and obtain typed references.
///
/// [`ProgramBuilder::build`] rejects the explicitly knowable assembly errors in
/// ADR-0003 without introspecting Component fields or behavior-dependent sends.
pub struct ProgramBuilder {
    program: Arc<()>,
    components: Vec<Box<dyn ErasedComponentKernel>>,
    ports: Vec<PortDeclaration>,
    bindings: Vec<Box<dyn ErasedPortBinding>>,
}

impl ProgramBuilder {
    /// Adds a Component instance and returns its typed logical address.
    ///
    /// Registration order is assembly detail, not an application execution
    /// order. The Component configuration may hold only logical wiring and typed
    /// references—not live runtime resources.
    pub fn component<C>(&mut self, id: ComponentId, component: C) -> ComponentRef<C>
    where
        C: Component,
    {
        let component_ref = ComponentRef {
            id: id.clone(),
            program: self.program.clone(),
            marker: PhantomData,
        };
        self.components
            .push(Box::new(ComponentKernel::new(id, component)));

        component_ref
    }

    /// Declares one named logical dependency on protocol `P`.
    ///
    /// Identity includes both `P` and `id`; registration does not select a
    /// provider. Multiple differently named Ports of the same protocol are
    /// first-class and may be bound independently.
    pub fn port<P>(&mut self, id: PortId) -> Port<P>
    where
        P: Protocol,
    {
        self.ports.push(PortDeclaration {
            protocol_type: TypeId::of::<P>(),
            protocol_type_name: std::any::type_name::<P>(),
            id: id.clone(),
            program: self.program.clone(),
        });
        Port {
            id,
            program: self.program.clone(),
            marker: PhantomData,
        }
    }

    /// Binds one exact named Port to a provider Component.
    ///
    /// The selected Component's private Message type implements
    /// `From<P::Message>`. This makes Protocol acceptance a canonical type
    /// relationship declared by the provider Message rather than a conversion
    /// closure repeated at every binding site. Separate named `Port<P>` values
    /// may still bind independently to different provider instances.
    ///
    /// [`ProgramBuilder::build`] later requires exactly one binding for every
    /// declared Port and verifies that this provider belongs to the same
    /// builder. Port cycles remain legal.
    pub fn bind_port<P, C>(&mut self, port: &Port<P>, provider: &ComponentRef<C>)
    where
        P: Protocol,
        C: Component,
        C::Message: From<P::Message>,
    {
        self.bindings.push(Box::new(PortBinding {
            port: port.clone(),
            provider: provider.clone(),
        }));
    }

    /// Validates explicit assembly facts and finishes the Program blueprint.
    pub fn build(self) -> Result<Program, ProgramBuildError> {
        let mut component_ids = std::collections::HashSet::new();
        for component in &self.components {
            if !component_ids.insert(component.id().clone()) {
                return Err(ProgramBuildError::DuplicateComponentId(
                    component.id().clone(),
                ));
            }
        }

        let mut ports = std::collections::HashSet::new();
        for port in &self.ports {
            let key = (port.protocol_type, port.id.clone());
            if !ports.insert(key) {
                return Err(ProgramBuildError::DuplicatePort {
                    protocol_type: port.protocol_type_name,
                    port: port.id.clone(),
                });
            }
            if !Arc::ptr_eq(&port.program, &self.program) {
                return Err(ProgramBuildError::ForeignPort {
                    protocol_type: port.protocol_type_name,
                    port: port.id.clone(),
                });
            }
        }

        for binding in &self.bindings {
            if !Arc::ptr_eq(binding.port_program(), &self.program) {
                return Err(ProgramBuildError::ForeignPort {
                    protocol_type: binding.protocol_type_name(),
                    port: binding.port_id().clone(),
                });
            }
            if !Arc::ptr_eq(binding.provider_program(), &self.program) {
                return Err(ProgramBuildError::ForeignProvider {
                    provider: binding.provider_id().clone(),
                });
            }
            if !self.components.iter().any(|component| {
                component.id() == binding.provider_id()
                    && component.component_type_id() == binding.provider_component_type()
            }) {
                return Err(ProgramBuildError::ProviderNotRegistered {
                    provider: binding.provider_id().clone(),
                });
            }
        }

        for port in &self.ports {
            let count = self
                .bindings
                .iter()
                .filter(|binding| {
                    binding.protocol_type() == port.protocol_type && binding.port_id() == &port.id
                })
                .count();
            if count != 1 {
                return Err(ProgramBuildError::PortBindingCount {
                    protocol_type: port.protocol_type_name,
                    port: port.id.clone(),
                    count,
                });
            }
        }

        Ok(Program {
            program: self.program,
            components: self.components,
            bindings: self.bindings,
        })
    }
}

/// Terminal live-world implementation for one concrete [`EffectDescriptor`]
/// type.
///
/// This is an explicitly impure boundary. The runtime owns and supervises the
/// returned future, then passes its result through the pure mapper stored in the
/// originating [`Command`]. Controlled execution does not call this Driver.
pub trait EffectDriver<D: EffectDescriptor>: Send + Sync + 'static {
    /// Executes one typed intent and returns its typed success or failure.
    fn execute(&self, descriptor: D) -> BoxFuture<Result<D::Output, D::Error>>;
}

/// Terminal live-world implementation for one concrete world-facing
/// [`SourceDescriptor`] type.
///
/// The returned future belongs to the runtime's structured-concurrency scope.
/// It may emit many items through [`SourceSink::emit`], then should terminate
/// explicitly with [`SourceSink::fail`] or [`SourceSink::end`]. If the sink
/// returns [`DriverStopped`], the Driver should promptly release its resources
/// and return. Returning normally without an accepted terminal call is one
/// implicit normal end for the active Source generation.
pub trait SourceDriver<D: SourceDescriptor>: Send + Sync + 'static {
    /// Runs one active instance of the source descriptor.
    fn run(&self, descriptor: D, sink: SourceSink<D>) -> BoxFuture<()>;
}

/// Runtime-owned output channel supplied to a live [`SourceDriver`].
///
/// The sink translates Driver activity into [`SourceEvent`] values for the
/// subscription mapper. It offers no path to Component state.
pub struct SourceSink<D: SourceDescriptor> {
    inner: Arc<live_runtime::SourceSinkCore>,
    marker: PhantomData<fn() -> D>,
}

impl<D: SourceDescriptor> SourceSink<D> {
    pub(crate) fn runtime(inner: Arc<live_runtime::SourceSinkCore>) -> Self {
        Self {
            inner,
            marker: PhantomData,
        }
    }

    /// Emits one item while leaving the source active.
    ///
    /// `DriverStopped` means the owning subscription or runtime scope no longer
    /// accepts events.
    pub async fn emit(&self, item: D::Item) -> Result<(), DriverStopped> {
        self.inner.send(
            ErasedSourceEvent::typed::<D>(SourceEvent::Item(item)),
            false,
        )
    }

    /// Emits a terminal source failure.
    pub async fn fail(&self, error: D::Error) -> Result<(), DriverStopped> {
        self.inner.send(
            ErasedSourceEvent::typed::<D>(SourceEvent::Failed(error)),
            true,
        )
    }

    /// Emits normal terminal completion.
    pub async fn end(&self) -> Result<(), DriverStopped> {
        self.inner
            .send(ErasedSourceEvent::typed::<D>(SourceEvent::Ended), true)
    }
}

/// Signal that runtime ownership of a live Driver has ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DriverStopped;

impl fmt::Display for DriverStopped {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the runtime stopped the Driver")
    }
}

impl Error for DriverStopped {}

/// Topology-neutral runtime or controlled-harness diagnostic.
///
/// Samara exposes stable context for a faulting Component, descriptor, and work
/// occurrence while leaving the broader runtime taxonomy provisional.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeError(Arc<RuntimeErrorKind>);

#[derive(Debug, PartialEq, Eq)]
enum RuntimeErrorKind {
    Fault {
        component: ComponentId,
        descriptor_type: Option<&'static str>,
        work: TraceId,
        reason: Arc<str>,
    },
    Harness(Arc<str>),
}

impl RuntimeError {
    pub(crate) fn fault(
        component: ComponentId,
        descriptor_type: Option<&'static str>,
        work: TraceId,
        reason: impl Into<Arc<str>>,
    ) -> Self {
        Self(Arc::new(RuntimeErrorKind::Fault {
            component,
            descriptor_type,
            work,
            reason: reason.into(),
        }))
    }

    pub(crate) fn harness(reason: impl Into<Arc<str>>) -> Self {
        Self(Arc::new(RuntimeErrorKind::Harness(reason.into())))
    }

    /// Component at which execution faulted, if this is a runtime fault.
    pub fn component(&self) -> Option<&ComponentId> {
        match self.0.as_ref() {
            RuntimeErrorKind::Fault { component, .. } => Some(component),
            RuntimeErrorKind::Harness(_) => None,
        }
    }

    /// Concrete terminal descriptor type involved in a runtime fault.
    pub fn descriptor_type(&self) -> Option<&'static str> {
        match self.0.as_ref() {
            RuntimeErrorKind::Fault {
                descriptor_type, ..
            } => *descriptor_type,
            RuntimeErrorKind::Harness(_) => None,
        }
    }

    /// Structural trace identity of the work occurrence that faulted.
    pub fn work_occurrence(&self) -> Option<TraceId> {
        match self.0.as_ref() {
            RuntimeErrorKind::Fault { work, .. } => Some(*work),
            RuntimeErrorKind::Harness(_) => None,
        }
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.as_ref() {
            RuntimeErrorKind::Fault {
                component,
                descriptor_type,
                work,
                reason,
            } => write!(
                formatter,
                "Component {component:?} faulted at work {work:?}{}: {reason}",
                descriptor_type
                    .map(|name| format!(" ({name})"))
                    .unwrap_or_default()
            ),
            RuntimeErrorKind::Harness(reason) => formatter.write_str(reason),
        }
    }
}

impl Error for RuntimeError {}

/// Binds a topology-neutral [`Program`] to live Tokio world Drivers.
///
/// Binding is assembly-time work: Components still see only typed effect and
/// source descriptors. Missing initial bindings and duplicate or ambiguous
/// bindings make [`LiveRuntimeBuilder::build`] fail. A missing binding first
/// reached through dynamic work faults the running scope.
pub struct LiveRuntimeBuilder {
    program: Program,
    bindings: live_runtime::LiveBindings,
}

/// Fully assembled live program that will use real Tokio scheduling, time, and
/// world Drivers.
///
/// Live execution preserves per-Component serialization and causality, but does
/// not promise deterministic order between independent events.
pub struct LiveRuntime {
    program: Program,
    bindings: live_runtime::LiveBindings,
    scope: Arc<live_runtime::LiveScope>,
    receiver: tokio::sync::mpsc::UnboundedReceiver<live_runtime::LiveEvent>,
}

impl LiveRuntime {
    /// Starts live assembly for a topology-neutral program blueprint.
    pub fn builder(program: Program) -> LiveRuntimeBuilder {
        LiveRuntimeBuilder {
            program,
            bindings: live_runtime::LiveBindings::new(),
        }
    }

    /// Creates external ingress for one typed Component.
    ///
    /// Unlike [`ComponentRef`], a handle is a live runtime capability and must
    /// never be placed in a Component or passed to its transition.
    pub fn handle<C: Component>(
        &self,
        component: &ComponentRef<C>,
    ) -> Result<ComponentHandle<C>, RuntimeError> {
        if !Arc::ptr_eq(&component.program, &self.program.program)
            || !self.program.components.iter().any(|kernel| {
                kernel.id() == component.id() && kernel.component_type_id() == TypeId::of::<C>()
            })
        {
            return Err(RuntimeError::harness(
                "live Component handle target is not part of this Program",
            ));
        }
        Ok(ComponentHandle {
            component: component.clone(),
            scope: self.scope.clone(),
        })
    }

    /// Starts the assembled program in a runtime-owned structured scope.
    ///
    /// The returned [`RuntimeTask`] is the ownership handle used to shut down and
    /// account for all Samara-authorized work.
    pub fn spawn(self) -> RuntimeTask {
        let scope = self.scope.clone();
        let core =
            live_runtime::LiveCore::new(self.program, self.bindings, self.scope, self.receiver);
        RuntimeTask {
            scope,
            join: Some(tokio::spawn(core.run())),
        }
    }
}

impl LiveRuntimeBuilder {
    /// Binds a logical [`StreamDescriptor`] to one concrete Tokio receiver.
    ///
    /// The receiver is unique live-world state and therefore stays out of the
    /// Component and [`Program`]. Closing it produces [`SourceEvent::Ended`].
    pub fn bind_mpsc<T: Send + 'static>(
        mut self,
        stream: StreamDescriptor<T>,
        receiver: tokio::sync::mpsc::Receiver<T>,
    ) -> Self {
        live_runtime::bind_mpsc(&mut self.bindings, stream, receiver);
        self
    }

    /// Registers the live Driver for effect descriptor type `D`.
    pub fn bind_effect<D, Driver>(mut self, driver: Driver) -> Self
    where
        D: EffectDescriptor,
        Driver: EffectDriver<D>,
    {
        self.bindings.bind_effect::<D, Driver>(driver);
        self
    }

    /// Registers the live Driver for world-facing source descriptor type `D`.
    ///
    /// Pure source composition such as [`Framed`] is evaluated above this
    /// binding, so Drivers operate on raw world events rather than application
    /// messages.
    pub fn bind_source<D, Driver>(mut self, driver: Driver) -> Self
    where
        D: SourceDescriptor,
        Driver: SourceDriver<D>,
    {
        self.bindings.bind_source::<D, Driver>(driver);
        self
    }

    /// Registers Samara's first-party one-connection Tokio TCP Driver.
    ///
    /// Components declare [`TcpBytes`] values and controlled execution binds
    /// that same terminal descriptor type with `control_source::<TcpBytes>()`.
    /// The live Driver performs no DNS, retry, reconnect, or framing.
    pub fn bind_tcp(mut self) -> Self {
        live_runtime::bind_tcp(&mut self.bindings);
        self
    }

    /// Registers Samara's first-party pooled HTTP Effect Driver.
    ///
    /// Components issue raw [`HttpRequest`] descriptors and receive complete
    /// [`HttpResponse`] values or typed [`HttpError`] data. One reusable client
    /// and connection pool are retained by this binding. The v0 Driver follows
    /// no redirects, performs no retries, uses no system proxy, performs no
    /// automatic content decompression, and applies no status or body-decoding
    /// policy. It supplies `Accept: */*` only when the descriptor omits
    /// `Accept`. Controlled execution uses the same descriptor through
    /// `control_effect::<HttpRequest>()` without network access.
    pub fn bind_http(mut self) -> Self {
        live_runtime::bind_http(&mut self.bindings);
        self
    }

    /// Registers Samara's first-party Tokio standard-output Drivers.
    ///
    /// [`PrintStdout`] and [`PrintStderr`] each attempt to write their complete
    /// text and flush the selected stream. Host I/O errors are deliberately
    /// discarded, so these high-level effects are infallible from the
    /// Component's perspective. Within this binding, each stream is serialized
    /// independently; this method adds no ordering guarantee among otherwise
    /// independent effects, between stdout and stderr, or relative to direct
    /// process writes.
    pub fn bind_stdio(mut self) -> Self {
        live_runtime::bind_stdio(&mut self.bindings);
        self
    }

    /// Validates bindings and finishes live assembly.
    pub fn build(self) -> Result<LiveRuntime, RuntimeError> {
        self.bindings.validate()?;
        for component in &self.program.components {
            component.validate_initial_live_bindings(&self.bindings)?;
        }
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let scope = Arc::new(live_runtime::LiveScope::new(sender));
        Ok(LiveRuntime {
            program: self.program,
            bindings: self.bindings,
            scope,
            receiver,
        })
    }
}

/// Live external sender for one typed Component.
///
/// Application Components normally communicate through [`Command::notify`] and
/// [`Command::request`], or through lower-level [`Command::send`] when tight coupling is
/// intentional. This capability is for surrounding Tokio code at the Samara
/// program boundary.
///
/// A live handle deliberately exposes no Model access or mutation capability:
///
/// ```compile_fail
/// use samara::{Component, ComponentHandle};
///
/// fn replace_model<C: Component>(handle: &mut ComponentHandle<C>, model: C::Model) {
///     *handle.model_mut() = model;
/// }
/// ```
pub struct ComponentHandle<C: Component> {
    component: ComponentRef<C>,
    scope: Arc<live_runtime::LiveScope>,
}

impl<C: Component> Clone for ComponentHandle<C> {
    fn clone(&self) -> Self {
        Self {
            component: self.component.clone(),
            scope: self.scope.clone(),
        }
    }
}

impl<C: Component> ComponentHandle<C> {
    /// Submits a message to the live runtime.
    ///
    /// Successful return means accepted for runtime-managed delivery, not that
    /// the target transition has completed.
    pub async fn send(&self, message: C::Message) -> Result<(), RuntimeError> {
        self.scope.accept_ingress(live_runtime::LiveEvent::ingress(
            live_runtime::LiveIngress {
                target: self.component.id().clone(),
                target_message_type: TypeId::of::<C::Message>(),
                message_type_name: std::any::type_name::<C::Message>(),
                message: Box::new(message),
            },
        ))
    }
}

/// Ownership handle for a running live Samara program.
///
/// Dropping or shutting down this scope must not leave detached commands,
/// subscriptions, or Driver work.
pub struct RuntimeTask {
    scope: Arc<live_runtime::LiveScope>,
    join: Option<tokio::task::JoinHandle<Result<ShutdownReport, RuntimeError>>>,
}

impl RuntimeTask {
    /// Ends the program using the requested shutdown policy and returns
    /// work-accounting evidence.
    pub async fn shutdown(mut self, mode: Shutdown) -> Result<ShutdownReport, RuntimeError> {
        let cutoff = self.scope.begin_shutdown(mode);
        let result = self
            .join
            .as_mut()
            .expect("a RuntimeTask owns one live runtime task")
            .await
            .map_err(|_| RuntimeError::harness("live runtime owner task panicked"));
        // Keep the JoinHandle inside `self` across the await. If a host timeout
        // drops this shutdown future, `RuntimeTask::drop` can still abort the
        // owner instead of detaching it. A completed owner no longer needs that
        // guard.
        self.join.take();
        let result = result?;
        match result {
            Ok(report) => {
                cutoff?;
                Ok(report)
            }
            Err(error) => Err(error),
        }
    }
}

impl Drop for RuntimeTask {
    fn drop(&mut self) {
        if let Some(join) = self.join.take() {
            // Drop cannot asynchronously join the owner, but it can atomically
            // close admission before aborting it. Dropping `LiveCore` then
            // drops its `JoinSet`, which aborts every owned child task.
            let _ = self.scope.begin_shutdown(Shutdown::Cancel);
            join.abort();
        }
    }
}

/// Policy for runtime-owned work during live shutdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shutdown {
    /// Stop Sources and recursively finish accepted and causally emitted finite
    /// work. Drain has no implicit deadline and may wait forever.
    Drain,
    /// Stop application driving, cancel owned work, and emit no synthetic
    /// application outcomes solely because the scope ended.
    Cancel,
}

/// Structured-concurrency accounting returned after a runtime scope closes.
///
/// The exact unit counted as one piece of work remains provisional; the key
/// invariant is that `remaining` must be zero after successful live shutdown or
/// controlled cancellation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShutdownReport {
    /// Runtime-owned work units that completed while the scope was closing.
    pub completed: usize,
    /// Runtime-owned work units that were cancelled while the scope was closing.
    pub cancelled: usize,
    /// Work units still owned after scope closure returned.
    pub remaining: usize,
    /// Immediately runnable semantic obligations after scope closure.
    pub pending_now: usize,
    /// Deferred semantic obligations after scope closure.
    pub pending_later: usize,
}

impl ShutdownReport {
    /// Returns whether scope closure left no owned work behind.
    pub fn is_clean(&self) -> bool {
        self.remaining == 0
    }
}

/// Declares which effect and source boundaries the controlled world may script.
///
/// Controlled assembly never falls back to a live Driver. Missing controlled
/// behavior must fail explicitly so a test cannot accidentally touch the world.
pub struct ControlledRuntimeBuilder {
    program: Program,
    bindings: controlled_runtime::ControlledBindings,
}

/// Deterministic, synchronously driven execution of a [`Program`].
///
/// Components, commands, subscriptions, decoders, and declared boundary contracts
/// are identical to live execution. The difference is runtime decisions: tests
/// supply external events, effect outcomes, and logical-time progression.
pub struct ControlledRuntime {
    core: ControlledCore,
}

impl ControlledRuntime {
    /// Starts controlled assembly for a topology-neutral program blueprint.
    pub fn builder(program: Program) -> ControlledRuntimeBuilder {
        ControlledRuntimeBuilder {
            program,
            bindings: controlled_runtime::ControlledBindings::new(),
        }
    }

    /// Enqueues a typed message at a Component boundary.
    ///
    /// The transition runs only when a drive method such as
    /// [`ControlledRuntime::run_until_idle`] is called.
    pub fn send<C: Component>(
        &mut self,
        component: &ComponentRef<C>,
        message: C::Message,
    ) -> Result<(), RuntimeError> {
        self.core.send(component, message)
    }

    /// Emits one item through a controlled [`StreamDescriptor`].
    pub fn emit_stream<T: Send + 'static>(
        &mut self,
        stream: &StreamDescriptor<T>,
        item: T,
    ) -> Result<(), RuntimeError> {
        self.core.emit_stream(stream, item)
    }

    /// Ends a controlled [`StreamDescriptor`] normally.
    ///
    /// Its subscription mapper receives [`SourceEvent::Ended`].
    pub fn close_stream<T: Send + 'static>(
        &mut self,
        stream: &StreamDescriptor<T>,
    ) -> Result<(), RuntimeError> {
        self.core.close_stream(stream)
    }

    /// Injects an event at the typed terminal source of an active subscription.
    ///
    /// For `Framed<TcpBytes, Decoder>`, controlled tests select the underlying
    /// terminal `TcpBytes` descriptor and inject raw chunks. Samara then runs the same decoder
    /// used by live execution before invoking the subscription mapper. Injection
    /// into an inactive identity or a non-terminal descriptor type is an error.
    pub fn emit_source<C, S>(
        &mut self,
        component: &ComponentRef<C>,
        id: &SubscriptionId,
        event: SourceEvent<S::Item, S::Error>,
    ) -> Result<(), RuntimeError>
    where
        C: Component,
        S: SourceDescriptor,
    {
        self.core.emit_source::<C, S>(component, id, event)
    }

    /// Returns a clone of the active source descriptor for inspection.
    ///
    /// Tests use this to verify subscription reconciliation and resource
    /// configuration without observing Driver-owned operational state.
    pub fn source_descriptor<C, S>(
        &self,
        component: &ComponentRef<C>,
        id: &SubscriptionId,
    ) -> Result<S, RuntimeError>
    where
        C: Component,
        S: SourceDescriptor,
    {
        self.core.source_descriptor(component, id)
    }

    /// Claims and returns the next pending effect of concrete type `E`.
    ///
    /// Selection follows the controlled scheduler's stable deterministic order,
    /// which is reproducibility machinery rather than a live ordering promise.
    /// The occurrence remains a pending semantic obligation until
    /// [`ControlledRuntime::complete`] or controlled cancellation, but its
    /// descriptor cannot be claimed a second time.
    pub fn next_effect<E: EffectDescriptor>(&mut self) -> Result<PendingEffect<E>, RuntimeError> {
        self.core.next_effect()
    }

    /// Supplies a terminal outcome for a previously intercepted effect.
    ///
    /// For [`Command::effect`], completion invokes the command's pure mapper and
    /// enqueues the resulting Message. For
    /// [`Command::effect_discarding_outcome`], it records the outcome and closes
    /// the obligation without scheduling a Message. Neither mode executes a
    /// live [`EffectDriver`].
    pub fn complete<E: EffectDescriptor>(
        &mut self,
        pending: PendingEffect<E>,
        outcome: EffectOutcome<E::Output, E::Error>,
    ) -> Result<(), RuntimeError> {
        self.core.complete(pending, outcome)
    }

    /// Processes immediately runnable work until the program is quiescent now.
    ///
    /// Future timers and open subscriptions remain pending and do not prevent
    /// return. This method does not advance logical time.
    pub fn run_until_idle(&mut self) -> Result<RunReport, RuntimeError> {
        self.core.run_until_idle()
    }

    /// Drains work due at the current instant, advances logical time by
    /// `duration`, and processes each newly reachable instant until immediate
    /// quiescence.
    pub fn advance(&mut self, duration: Duration) -> Result<RunReport, RuntimeError> {
        self.core.advance(duration)
    }

    /// Advances to the next scheduled logical instant and processes work there.
    ///
    /// Returns an error when no future scheduled work exists. Repeated calls are
    /// the primitive behind automatic time advancement.
    pub fn advance_to_next(&mut self) -> Result<RunReport, RuntimeError> {
        self.core.advance_to_next()
    }

    /// Closes this controlled scope by cancelling all remaining owned work.
    ///
    /// Consuming the runtime prevents the harness from supplying any further
    /// controlled input. Cancellation and accounting proceed in deterministic
    /// controlled-runtime order, covering messages, subscriptions, timers,
    /// effects, and requests owned by the scope. Successful return guarantees that
    /// [`ShutdownReport::remaining`] is zero.
    ///
    /// This is lifecycle and diagnostic behavior only: it does not mean the
    /// application reached a domain-defined completion state. A test that needs
    /// a Component to observe cancellation must explicitly supply the
    /// appropriate typed cancellation outcome before calling this method.
    pub fn cancel(self) -> Result<ShutdownReport, RuntimeError> {
        self.core.cancel()
    }

    /// Borrows the current model while controlled execution is paused.
    ///
    /// Live execution deliberately has no equivalent shared model handle.
    pub fn state<C: Component>(
        &self,
        component: &ComponentRef<C>,
    ) -> Result<&C::Model, RuntimeError> {
        self.core.state(component)
    }

    /// Returns the currently owned semantic obligations without driving.
    pub fn pending_work(&self) -> PendingWork {
        self.core.pending_work()
    }

    /// Returns the deterministic structured semantic trace accumulated so far.
    ///
    /// Trace observation has no callback or feedback path into Components.
    pub fn trace(&self) -> &[TraceRecord] {
        self.core.trace()
    }
}

impl ControlledRuntimeBuilder {
    /// Allows tests to script one logical [`StreamDescriptor`].
    pub fn control_stream<T: Send + 'static>(mut self, stream: StreamDescriptor<T>) -> Self {
        self.bindings
            .exact_sources
            .push(controlled_runtime::exact_source(stream));
        self
    }

    /// Allows tests to intercept and complete effect type `E`.
    pub fn control_effect<E: EffectDescriptor>(mut self) -> Self {
        self.bindings.effects.insert(TypeId::of::<E>());
        self
    }

    /// Allows tests to inject events for world-facing source type `S`.
    ///
    /// For a composed [`Framed`] source, register and inject the underlying
    /// source type so the decoder remains part of the program under test.
    pub fn control_source<S: SourceDescriptor>(mut self) -> Self {
        self.bindings.sources.insert(TypeId::of::<S>());
        self
    }

    /// Creates a paused deterministic runtime with the declared controls.
    ///
    /// Missing terminal behavior faults only when initialization or a later
    /// transition reaches that boundary, preserving an inspectable runtime and
    /// trace for diagnosis.
    pub fn build(self) -> Result<ControlledRuntime, RuntimeError> {
        Ok(ControlledRuntime {
            core: ControlledCore::new(self.program, self.bindings),
        })
    }
}

/// Typed effect claimed before any live world interaction occurs.
///
/// This token must be returned to [`ControlledRuntime::complete`] on the same
/// runtime or the effect remains pending until controlled cancellation.
#[must_use = "a PendingEffect remains outstanding until completed or cancelled"]
pub struct PendingEffect<E: EffectDescriptor> {
    /// Original effect intent emitted by the Component transition.
    pub intent: E,
    id: u64,
    program: Arc<()>,
}

/// Snapshot of semantic obligations owned by a controlled runtime.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PendingWork {
    /// Accepted Component Messages and due timers ready to run.
    pub pending_now: usize,
    /// Effects, future timers, active Sources, and outstanding Requests.
    pub pending_later: usize,
}

/// Work summary returned by one controlled-runtime drive operation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunReport {
    /// Component transitions committed by this drive operation.
    pub transitions: usize,
    /// Work still immediately runnable when the operation stopped.
    pub pending_now: usize,
    /// Work waiting for controlled input or a later logical instant.
    pub pending_later: usize,
}

/// Runtime-controlled logical instant used by controlled scheduling and trace.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LogicalTime(Duration);

impl LogicalTime {
    /// Returns the elapsed duration from the controlled runtime's origin.
    pub fn as_duration(self) -> Duration {
        self.0
    }
}

/// Identity of one in-memory controlled trace record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TraceId(u64);

impl TraceId {
    /// Returns the deterministic run-local numeric identity.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Common envelope for one structural controlled semantic event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceRecord {
    /// Run-local record identity.
    pub id: TraceId,
    /// Logical instant at which the event occurred.
    pub at: LogicalTime,
    /// Immediate causal parent; absent only for initialization and harness-input roots.
    pub cause: Option<TraceId>,
    /// Structural semantic event.
    pub event: TraceEvent,
}

/// Finite-work kind recorded without copying application payloads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceCommandKind {
    /// A terminal EffectDescriptor was requested.
    Effect,
    /// A direct Component Message delivery was requested.
    Send,
    /// A one-way Protocol notification was requested.
    Notify,
    /// A correlated Protocol Request was issued.
    Request,
    /// A provider emitted a correlated Reply.
    Reply,
    /// A Message was scheduled against logical time.
    Timer,
}

/// Source-maintenance decision committed after a Component transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubscriptionAction {
    /// A newly desired Source realization started.
    Started,
    /// An equal descriptor retained its Source and adopted the latest mapper.
    Retained,
    /// A changed descriptor withdrew one generation and started another.
    Replaced,
    /// Removed desire cancelled the active Source.
    Cancelled,
}

/// Structural terminal shape of one effect completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectOutcomeKind {
    /// The effect succeeded.
    Succeeded,
    /// The effect failed with typed Error data.
    Failed,
    /// Runtime ownership cancelled the effect.
    Cancelled,
}

/// Always-collected structural semantic event from controlled execution.
///
/// This v0 trace records concrete Rust types, targets, lifecycle, time, and
/// causation without copying descriptor or Message payloads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceEvent {
    /// Parentless initialization root for one Component.
    Initialization {
        /// Component entering the controlled Program.
        component: ComponentId,
    },
    /// Parentless controlled Message input accepted from the harness.
    ControlledMessageInput {
        /// Target Component.
        component: ComponentId,
        /// Diagnostic Rust Message type name.
        message_type: &'static str,
    },
    /// Parentless controlled terminal Source input accepted from the harness.
    ControlledSourceInput {
        /// Component owning the Source.
        component: ComponentId,
        /// Stable Component-local Subscription identity.
        subscription: SubscriptionId,
        /// Terminal descriptor type reached after SourcePlan lowering.
        source_descriptor_type: &'static str,
        /// Structural input event shape.
        event: SourceEventKind,
    },
    /// A Component committed one message transition.
    Transition {
        /// Component that owned the transition.
        component: ComponentId,
        /// Diagnostic Rust type name of the delivered message.
        message_type: &'static str,
    },
    /// One flattened Command occurrence emitted by a transition or initialization.
    CommandEmitted {
        /// Component that owns the Command.
        component: ComponentId,
        /// Structural finite-work category.
        kind: TraceCommandKind,
        /// Concrete descriptor, operation, reply, or Message type when applicable.
        detail_type: &'static str,
        /// Direct target Component, if this Command uses one.
        target_component: Option<ComponentId>,
        /// Named target Port, if this Command uses one.
        target_port: Option<PortId>,
    },
    /// Post-transition Subscription reconciliation decision.
    SubscriptionLifecycle {
        /// Component owning the Subscription.
        component: ComponentId,
        /// Stable Component-local Subscription identity.
        subscription: SubscriptionId,
        /// Complete desired SourceDescriptor type.
        source_descriptor_type: &'static str,
        /// Lifecycle action selected by reconciliation.
        action: SubscriptionAction,
    },
    /// One event emerged from the ordered SourcePlan Layers.
    SourceEventMapped {
        /// Component owning the Source.
        component: ComponentId,
        /// Stable Component-local Subscription identity.
        subscription: SubscriptionId,
        /// Complete composed descriptor type presented to the mapper.
        source_descriptor_type: &'static str,
        /// Structural outer event shape.
        event: SourceEventKind,
    },
    /// Old-generation Source work was rejected before a transition began.
    StaleSourceWorkDropped {
        /// Component that would have received the work.
        component: ComponentId,
        /// Stable Component-local Subscription identity.
        subscription: SubscriptionId,
        /// Whether the stale occurrence was an event or an already-mapped Message.
        mapped_message: bool,
    },
    /// One scripted terminal effect outcome was accepted.
    EffectOutcome {
        /// Command occurrence whose pending effect this input resolves.
        effect: TraceId,
        /// Component that requested the effect.
        component: ComponentId,
        /// Concrete EffectDescriptor type.
        effect_type: &'static str,
        /// Structural terminal outcome shape.
        outcome: EffectOutcomeKind,
    },
    /// One successful correlated Request resolved to `Replied`.
    RequestOutcome {
        /// Requesting Component.
        component: ComponentId,
        /// Concrete Reply type.
        reply_type: &'static str,
    },
    /// A due logical timer released its Message.
    TimerFired {
        /// Component receiving the scheduled Message.
        component: ComponentId,
        /// Diagnostic Rust Message type.
        message_type: &'static str,
    },
    /// A terminal boundary or runtime invariant faulted the controlled run.
    RuntimeFault {
        /// Component whose work encountered the fault.
        component: ComponentId,
        /// Command or lifecycle trace occurrence that faulted.
        work: TraceId,
        /// Concrete terminal descriptor type when applicable.
        descriptor_type: &'static str,
        /// Topology-neutral diagnostic explanation.
        reason: Arc<str>,
    },
}

#[cfg(test)]
mod protocol_macro_tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    struct Snapshot(u8);

    protocol! {
        type MacroProtocol => enum MacroProtocolMessage {
            SnapshotRequest -> Snapshot,
            Observation(u8),
            Refresh,
            Lookup(u16) -> Snapshot,
        }
    }

    protocol! {
        type UnitReplyProtocol => enum UnitReplyProtocolMessage {
            Reset -> (),
        }
    }

    #[test]
    fn generated_payload_notification_is_flattened() {
        fn assert_notification<N>()
        where
            N: Notification<MacroProtocol>,
        {
        }

        assert_notification::<Observation>();
        let message = <Observation as Notification<MacroProtocol>>::into_message(Observation(7));

        match message {
            MacroProtocolMessage::Observation(observed) => assert_eq!(observed, 7),
            _ => panic!("notification became a different operation"),
        }
    }

    #[test]
    fn generated_unit_notification_is_flattened() {
        let message = <Refresh as Notification<MacroProtocol>>::into_message(Refresh);

        assert!(matches!(message, MacroProtocolMessage::Refresh));
    }

    #[test]
    fn generated_unit_request_preserves_typed_reply_association() {
        fn assert_reply_type<R>()
        where
            R: Request<MacroProtocol, Reply = Snapshot>,
        {
        }

        assert_reply_type::<SnapshotRequest>();
        let message = <SnapshotRequest as Request<MacroProtocol>>::into_message(
            SnapshotRequest,
            ReplyTo::<Snapshot>::sketch(41),
        );

        match message {
            MacroProtocolMessage::SnapshotRequest(invocation) => {
                let SnapshotRequest = invocation.request;
                assert_eq!(invocation.reply_to.correlation, 41);
                let _: ReplyTo<Snapshot> = invocation.reply_to;
            }
            _ => panic!("request became a different operation"),
        }
    }

    #[test]
    fn generated_payload_request_preserves_its_operation_value() {
        fn assert_reply_type<R>()
        where
            R: Request<MacroProtocol, Reply = Snapshot>,
        {
        }

        assert_reply_type::<Lookup>();
        let message = <Lookup as Request<MacroProtocol>>::into_message(
            Lookup(17),
            ReplyTo::<Snapshot>::sketch(42),
        );

        match message {
            MacroProtocolMessage::Lookup(invocation) => {
                assert_eq!(invocation.request.0, 17);
                assert_eq!(invocation.reply_to.correlation, 42);
            }
            _ => panic!("request became a different operation"),
        }
    }

    #[test]
    fn explicit_unit_reply_is_still_a_request() {
        fn assert_unit_reply<R>()
        where
            R: Request<UnitReplyProtocol, Reply = ()>,
        {
        }

        assert_unit_reply::<Reset>();
    }
}

#[cfg(test)]
mod source_plan_tests {
    use std::convert::Infallible;

    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct WrongTerminal;

    impl SourceDescriptor for WrongTerminal {
        type Item = u64;
        type Error = Infallible;
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct MalformedInternalInner;

    impl SourceDescriptor for MalformedInternalInner {
        type Item = Vec<u8>;
        type Error = Infallible;

        fn __samara_source_plan(&self, _: source_plan_private::LowerToken) -> SourcePlan {
            WrongTerminal.__samara_source_plan(source_plan_private::LOWER_TOKEN)
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct PassthroughDecoder;

    impl Decoder for PassthroughDecoder {
        type Chunk = Vec<u8>;
        type Frame = Vec<u8>;
        type Error = Infallible;
        type State = ();

        fn start(&self) -> Self::State {}

        fn push(
            &self,
            _state: &mut Self::State,
            chunk: Self::Chunk,
        ) -> Result<Vec<Self::Frame>, Self::Error> {
            Ok(vec![chunk])
        }

        fn finish(&self, _state: &mut Self::State) -> Result<Vec<Self::Frame>, Self::Error> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn internal_plan_validation_rejects_a_mismatched_terminal_event_type() {
        let plan = MalformedInternalInner.__samara_source_plan(source_plan_private::LOWER_TOKEN);

        assert_eq!(plan.terminal_type_id(), TypeId::of::<WrongTerminal>());
        assert!(!plan.accepts_output_event_type(TypeId::of::<SourceEvent<Vec<u8>, Infallible>>()));
    }

    #[test]
    fn internal_plan_validation_preserves_invalidity_through_a_built_in_layer() {
        let descriptor = Framed::new(MalformedInternalInner, PassthroughDecoder);
        let plan = descriptor.__samara_source_plan(source_plan_private::LOWER_TOKEN);

        assert_eq!(plan.terminal_type_id(), TypeId::of::<WrongTerminal>());
        assert!(!plan.accepts_output_event_type(TypeId::of::<
            SourceEvent<Vec<u8>, FramedError<Infallible, Infallible>>,
        >()));
    }
}
