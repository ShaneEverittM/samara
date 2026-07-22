#![allow(dead_code)]
#![warn(missing_docs)]

//! A compiler-checked sketch of Samara's possible public API.
//!
//! # Status
//!
//! This crate is a design artifact, not a runtime implementation. Runtime
//! methods intentionally return placeholder errors. The public shapes exist so
//! the consumers in `src/bin` can expose awkward ownership, typing, and
//! ergonomics choices before Samara commits to them.
//!
//! # Mental model
//!
//! 1. A [`Component`] owns a private model and accepts typed messages.
//! 2. [`Component::update`] changes that model synchronously and returns a
//!    [`Cmd`] describing finite work. It receives no runtime or I/O capability.
//! 3. [`Component::subscriptions`] declares the ongoing [`Source`] values the
//!    current model wants active. Declaring a subscription does not start work.
//! 4. A [`Program`] assembles Components, typed references, and named [`Port`]
//!    dependencies. Ports expose provider-neutral protocols rather than a
//!    provider Component's private message enum.
//! 5. [`LiveRuntime`] binds effect and source descriptors to Tokio adapters.
//!    [`ControlledRuntime`] exposes the same boundaries as scripted inputs for
//!    deterministic tests.
//!
//! Commands and subscriptions contain explicit, typed intent. The runtime owns
//! their asynchronous execution and returns behaviorally relevant outcomes as
//! messages through [`EffectEvent`], [`SourceEvent`], and [`RequestOutcome`].

use std::{
    any::Any, error::Error, fmt, future::Future, marker::PhantomData, pin::Pin, sync::Arc,
    time::Duration,
};

/// Convenient imports for writing Components and assembling either runtime
/// profile.
pub mod prelude {
    pub use crate::{
        AdapterStopped, BoxFuture, CancelReason, Cmd, Component, ComponentHandle, ComponentId,
        ComponentRef, ControlledRuntime, Decoder, Effect, EffectAdapter, EffectEvent, Framed,
        FramedFailure, Incoming, Init, LiveRuntime, MpscSource, Notification, PendingEffect, Port,
        PortId, Program, ProgramBuilder, Protocol, ReplyTo, Request, RequestError, RequestOutcome,
        RunReport, RuntimeError, RuntimeTask, Shutdown, ShutdownReport, Source, SourceAdapter,
        SourceEvent, SourceSink, Subscription, SubscriptionId, Subscriptions, TraceEvent, protocol,
    };
}

/// A boxed, sendable future returned by a live effect or source adapter.
///
/// Returning the future to Samara transfers lifecycle ownership to the runtime;
/// adapters should not detach untracked Tokio tasks behind this boundary.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Stable logical identity of one Component instance in a [`Program`].
///
/// This identifies the application unit, not a Tokio task, mailbox, or thread.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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
/// [`EffectAdapter`] interprets the value, while controlled execution exposes it
/// as [`PendingEffect`] for scripted completion.
pub trait Effect: Send + 'static {
    /// Value produced when the effect succeeds.
    type Output: Send + 'static;
    /// Domain-facing failure produced by the effect interpreter.
    type Error: Send + 'static;
}

/// A cloneable, comparable description of an ongoing external event source.
///
/// Equality is semantic: the runtime uses it during subscription reconciliation
/// to decide whether an active source is unchanged. Operational state such as a
/// socket handle or partial input buffer must not live in this descriptor.
pub trait Source: Clone + PartialEq + fmt::Debug + Send + Sync + 'static {
    /// Item emitted while the source remains active.
    type Item: Send + 'static;
    /// Failure that terminates the active source instance.
    type Error: Send + 'static;
}

/// Behaviorally relevant outcome of one finite [`Effect`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EffectEvent<Output, EffectError> {
    /// The effect completed with its typed output.
    Succeeded(Output),
    /// The effect completed with its typed failure.
    Failed(EffectError),
    /// Runtime ownership ended the effect before completion.
    Cancelled(CancelReason),
}

/// Event delivered by an active [`Source`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceEvent<Item, SourceError> {
    /// The source emitted another typed item and remains active.
    Item(Item),
    /// The active source instance failed and terminated.
    Failed(SourceError),
    /// The active source ended normally.
    Ended,
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
pub struct Init<Model, Msg> {
    /// The Component's initial private state.
    pub model: Model,
    /// Finite work requested as the Component enters the program.
    pub cmd: Cmd<Msg>,
}

impl<Model, Msg> Init<Model, Msg> {
    /// Creates initialization with no startup command.
    pub fn new(model: Model) -> Self {
        Self {
            model,
            cmd: Cmd::none(),
        }
    }

    /// Adds finite work to request after the initial model is installed.
    pub fn with_cmd(mut self, cmd: Cmd<Msg>) -> Self {
        self.cmd = cmd;
        self
    }
}

/// Samara's topology-neutral unit of state and behavior.
///
/// A Component value may contain immutable logical wiring such as a
/// [`MpscSource`], [`Port`], or [`ComponentRef`], but it must not contain ambient
/// runtime, clock, I/O, or mutable state handles. The runtime owns `Model` and
/// guarantees that two calls to [`Component::update`] for the same Component
/// never overlap.
pub trait Component: Send + Sync + 'static {
    /// All mutable application state exclusively owned by this Component.
    type Model: Send + 'static;
    /// The only input allowed to trigger a state transition.
    type Msg: Send + 'static;

    /// Produces the initial model and any explicit startup work.
    fn init(&self) -> Init<Self::Model, Self::Msg>;

    /// Must be observationally pure. The mutable reference is exclusively owned
    /// for the duration of this non-overlapping transition.
    fn update(&self, model: &mut Self::Model, msg: Self::Msg) -> Cmd<Self::Msg>;

    /// Purely describes the ongoing sources desired by the current model.
    ///
    /// The runtime evaluates this after a committed transition and reconciles
    /// the returned set by [`SubscriptionId`] plus source equality. The default
    /// declares no ongoing work.
    fn subscriptions(&self, _model: &Self::Model) -> Subscriptions<Self::Msg> {
        Subscriptions::none()
    }
}

/// A provider-neutral set of values accepted through a [`Port`].
///
/// `Inbound` is normally a protocol-owned enum containing notification values
/// and typed [`Incoming`] requests. It is deliberately distinct from any
/// provider Component's private [`Component::Msg`] type. Program assembly maps
/// it into the selected provider through [`ProgramBuilder::bind_port`].
pub trait Protocol: Send + Sync + 'static {
    /// Complete provider-facing input vocabulary for this protocol.
    type Inbound: Send + 'static;
}

/// A one-way value belonging to protocol `P`.
///
/// Conversion to [`Protocol::Inbound`] is synchronous application logic and
/// must be pure and deterministic. Sending the returned value remains a
/// runtime-interpreted command rather than a direct provider call.
pub trait Notification<P: Protocol>: Send + 'static {
    /// Wraps this notification in the protocol's provider-facing vocabulary.
    fn into_inbound(self) -> P::Inbound;
}

/// A request belonging to protocol `P` with one statically known reply type.
///
/// The runtime creates an opaque [`ReplyTo`] value when interpreting
/// [`Cmd::request`]. This pure conversion places the request and token into the
/// protocol's [`Protocol::Inbound`] vocabulary for delivery to its bound
/// provider.
pub trait Request<P: Protocol>: Send + 'static {
    /// Reply value accepted for this particular request type.
    type Reply: Send + 'static;

    /// Wraps the request and runtime-owned reply token for provider delivery.
    fn into_inbound(self, reply_to: ReplyTo<Self::Reply>) -> P::Inbound;
}

/// Requester-visible terminal outcome of a correlated [`Cmd::request`].
///
/// The variants establish that request liveness is explicit Component input. The
/// default timeout policy, cancellation taxonomy, and exact point at which a
/// delivery becomes failed remain deliberately unresolved in this sketch.
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

/// Provisional topology-neutral failure reported through [`RequestOutcome`].
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
/// so [`Cmd::reply`] consumes the only authority presented to the provider.
/// Application code cannot construct a token; the runtime creates it while
/// interpreting [`Cmd::request`].
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
/// [`Protocol::Inbound`] enum variant. Provider Components inspect `request`
/// and pass `reply_to` to [`Cmd::reply`]; they never await or access runtime
/// state while handling it.
pub struct Incoming<P, R>
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

impl<P, R> Incoming<P, R>
where
    P: Protocol,
    R: Request<P>,
{
    /// Couples a request to the runtime-created reply authority.
    ///
    /// Protocol-owned [`Request::into_inbound`] implementations call this; the
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

impl<P, R> fmt::Debug for Incoming<P, R>
where
    P: Protocol,
    R: Request<P> + fmt::Debug,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Incoming")
            .field("request", &self.request)
            .field("reply_to", &self.reply_to)
            .finish()
    }
}

/// Declares the operation vocabulary of a provider-neutral protocol.
///
/// Each entry generates an ordinary, nameable operation type. The declaration
/// also generates a marker type, its [`Protocol`] implementation, a
/// provider-facing inbound enum, and the corresponding [`Notification`] and
/// [`Request`] conversions. Provider Components still match the generated
/// inbound enum themselves.
///
/// This is deliberately a small `macro_rules!` experiment. An entry with no
/// reply type is a notification; an entry followed by `-> Reply` is a request.
/// Entries may be unit operations or carry one tuple field. Notification
/// variants are flattened to that field, while request variants carry a typed
/// [`Incoming`]. A request with a unit reply must therefore spell `-> ()`.
/// Generated operation types and the inbound enum currently derive
/// [`Debug`](fmt::Debug).
///
/// The prototype does not yet accept per-operation attributes, multiple
/// fields, generics, or configurable derives. Those remain API questions; the
/// narrow grammar is not intended to settle them.
///
/// ```
/// use samara_api_sketch::prelude::*;
///
/// #[derive(Debug)]
/// pub struct HealthSnapshot;
///
/// protocol! {
///     pub type HealthProtocol => enum HealthInbound {
///         Changed(String),
///         Disconnected,
///         Read -> HealthSnapshot,
///     }
/// }
///
/// let changed = <Changed as Notification<HealthProtocol>>::into_inbound(
///     Changed("receiving".to_owned()),
/// );
/// assert!(matches!(changed, HealthInbound::Changed(status) if status == "receiving"));
/// ```
#[macro_export]
macro_rules! protocol {
    (
        $(#[$protocol_attr:meta])*
        $visibility:vis type $protocol:ident => enum $inbound:ident {
            $($entries:tt)*
        }
    ) => {
        $crate::protocol! {
            @parse
            [$(#[$protocol_attr])*]
            [$visibility]
            [$protocol]
            [$inbound]
            []
            []
            []
            $($entries)*
        }
    };

    // Finishes the TT muncher after accumulating generated operation items,
    // inbound variants, and trait implementations. Building the complete enum
    // here avoids relying on nested macros to expand to individual variants.
    (
        @parse
        [$(#[$protocol_attr:meta])*]
        [$visibility:vis]
        [$protocol:ident]
        [$inbound:ident]
        [$($operation_items:tt)*]
        [$($inbound_variants:tt)*]
        [$($implementations:tt)*]
    ) => {
        $(#[$protocol_attr])*
        $visibility struct $protocol;

        impl $crate::Protocol for $protocol {
            type Inbound = $inbound;
        }

        $($operation_items)*

        #[doc = concat!("Provider-facing envelope for `", stringify!($protocol), "`.")]
        #[derive(Debug)]
        $visibility enum $inbound {
            $($inbound_variants)*
        }

        $($implementations)*
    };

    // A payload-carrying request generates a tuple operation type, while its
    // inbound variant carries the complete operation plus correlation token.
    (
        @parse
        [$($protocol_attr:tt)*]
        [$visibility:vis]
        [$protocol:ident]
        [$inbound:ident]
        [$($operation_items:tt)*]
        [$($inbound_variants:tt)*]
        [$($implementations:tt)*]
        $operation:ident($payload:ty) -> $reply:ty,
        $($remaining:tt)*
    ) => {
        $crate::protocol! {
            @parse
            [$($protocol_attr)*]
            [$visibility]
            [$protocol]
            [$inbound]
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
                $($inbound_variants)*
                #[doc = concat!("Request operation `", stringify!($operation), "`.")]
                $operation($crate::Incoming<$protocol, $operation>),
            ]
            [
                $($implementations)*
                impl $crate::Request<$protocol> for $operation {
                    type Reply = $reply;

                    fn into_inbound(
                        self,
                        reply_to: $crate::ReplyTo<Self::Reply>,
                    ) -> $inbound {
                        $inbound::$operation($crate::Incoming::new(self, reply_to))
                    }
                }
            ]
            $($remaining)*
        }
    };

    // A unit request still has a tuple inbound variant because `Incoming`
    // carries the runtime-created reply authority.
    (
        @parse
        [$($protocol_attr:tt)*]
        [$visibility:vis]
        [$protocol:ident]
        [$inbound:ident]
        [$($operation_items:tt)*]
        [$($inbound_variants:tt)*]
        [$($implementations:tt)*]
        $operation:ident -> $reply:ty,
        $($remaining:tt)*
    ) => {
        $crate::protocol! {
            @parse
            [$($protocol_attr)*]
            [$visibility]
            [$protocol]
            [$inbound]
            [
                $($operation_items)*
                #[doc = concat!("Unit request operation in `", stringify!($protocol), "`.")]
                #[derive(Debug)]
                $visibility struct $operation;
            ]
            [
                $($inbound_variants)*
                #[doc = concat!("Request operation `", stringify!($operation), "`.")]
                $operation($crate::Incoming<$protocol, $operation>),
            ]
            [
                $($implementations)*
                impl $crate::Request<$protocol> for $operation {
                    type Reply = $reply;

                    fn into_inbound(
                        self,
                        reply_to: $crate::ReplyTo<Self::Reply>,
                    ) -> $inbound {
                        $inbound::$operation($crate::Incoming::new(self, reply_to))
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
        [$inbound:ident]
        [$($operation_items:tt)*]
        [$($inbound_variants:tt)*]
        [$($implementations:tt)*]
        $operation:ident($payload:ty),
        $($remaining:tt)*
    ) => {
        $crate::protocol! {
            @parse
            [$($protocol_attr)*]
            [$visibility]
            [$protocol]
            [$inbound]
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
                $($inbound_variants)*
                #[doc = concat!("Notification operation `", stringify!($operation), "`.")]
                $operation($payload),
            ]
            [
                $($implementations)*
                impl $crate::Notification<$protocol> for $operation {
                    fn into_inbound(self) -> $inbound {
                        let $operation(payload) = self;
                        $inbound::$operation(payload)
                    }
                }
            ]
            $($remaining)*
        }
    };

    // A unit notification becomes a unit inbound variant.
    (
        @parse
        [$($protocol_attr:tt)*]
        [$visibility:vis]
        [$protocol:ident]
        [$inbound:ident]
        [$($operation_items:tt)*]
        [$($inbound_variants:tt)*]
        [$($implementations:tt)*]
        $operation:ident,
        $($remaining:tt)*
    ) => {
        $crate::protocol! {
            @parse
            [$($protocol_attr)*]
            [$visibility]
            [$protocol]
            [$inbound]
            [
                $($operation_items)*
                #[doc = concat!("Unit notification operation in `", stringify!($protocol), "`.")]
                #[derive(Debug)]
                $visibility struct $operation;
            ]
            [
                $($inbound_variants)*
                #[doc = concat!("Notification operation `", stringify!($operation), "`.")]
                $operation,
            ]
            [
                $($implementations)*
                impl $crate::Notification<$protocol> for $operation {
                    fn into_inbound(self) -> $inbound {
                        $inbound::$operation
                    }
                }
            ]
            $($remaining)*
        }
    };
}

trait ErasedEffectCommand<Msg>: Send {
    fn intent(&self) -> &dyn Any;
    fn intent_type_name(&self) -> &'static str;
    fn mapper_type_name(&self) -> &'static str;
}

struct Perform<E, Map> {
    effect: E,
    map: Map,
}

impl<Msg, E, Map> ErasedEffectCommand<Msg> for Perform<E, Map>
where
    Msg: Send + 'static,
    E: Effect,
    Map: FnOnce(EffectEvent<E::Output, E::Error>) -> Msg + Send + 'static,
{
    fn intent(&self) -> &dyn Any {
        &self.effect
    }

    fn intent_type_name(&self) -> &'static str {
        std::any::type_name::<E>()
    }

    fn mapper_type_name(&self) -> &'static str {
        std::any::type_name::<Map>()
    }
}

trait ErasedSendCommand: Send {
    fn target(&self) -> &ComponentId;
    fn message_type_name(&self) -> &'static str;
}

struct SendTo<C: Component> {
    target: ComponentRef<C>,
    msg: C::Msg,
}

impl<C: Component> ErasedSendCommand for SendTo<C> {
    fn target(&self) -> &ComponentId {
        self.target.id()
    }

    fn message_type_name(&self) -> &'static str {
        std::any::type_name::<C::Msg>()
    }
}

trait ErasedNotificationCommand: Send {
    fn port(&self) -> &PortId;
    fn notification(&self) -> &dyn Any;
    fn notification_type_name(&self) -> &'static str;
    fn protocol_type_name(&self) -> &'static str;
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

    fn notification(&self) -> &dyn Any {
        &self.notification
    }

    fn notification_type_name(&self) -> &'static str {
        std::any::type_name::<N>()
    }

    fn protocol_type_name(&self) -> &'static str {
        std::any::type_name::<P>()
    }
}

trait ErasedRequestCommand<Msg>: Send {
    fn port(&self) -> &PortId;
    fn request(&self) -> &dyn Any;
    fn request_type_name(&self) -> &'static str;
    fn reply_type_name(&self) -> &'static str;
    fn protocol_type_name(&self) -> &'static str;
    fn mapper_type_name(&self) -> &'static str;
}

struct RequestCommand<P, R, Map>
where
    P: Protocol,
    R: Request<P>,
{
    port: Port<P>,
    request: R,
    map: Map,
}

impl<Msg, P, R, Map> ErasedRequestCommand<Msg> for RequestCommand<P, R, Map>
where
    Msg: Send + 'static,
    P: Protocol,
    R: Request<P>,
    Map: FnOnce(RequestOutcome<R::Reply>) -> Msg + Send + 'static,
{
    fn port(&self) -> &PortId {
        self.port.id()
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

    fn protocol_type_name(&self) -> &'static str {
        std::any::type_name::<P>()
    }

    fn mapper_type_name(&self) -> &'static str {
        std::any::type_name::<Map>()
    }
}

trait ErasedReplyCommand: Send {
    fn reply(&self) -> &dyn Any;
    fn reply_type_name(&self) -> &'static str;
}

struct Reply<Reply> {
    reply_to: ReplyTo<Reply>,
    reply: Reply,
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
}

/// A value describing finite work; never the work itself.
///
/// `Cmd<Msg>` can hold heterogeneous typed [`Effect`] values because the
/// runtime preserves their concrete types internally. Effect result mappers are
/// synchronous application logic and must be pure and deterministic. Mapper
/// object identity is not part of command semantics.
///
/// Commands intentionally do not implement `Clone`, `Debug`, or `PartialEq`:
/// they may contain one-shot result mappers. Transition tests inspect concrete
/// intent through [`Cmd::effect_intent`], [`Cmd::notification_intents`], and
/// [`Cmd::request_intents`], then test pure mappers with equivalent outcomes.
pub struct Cmd<Msg>(CmdKind<Msg>);

enum CmdKind<Msg> {
    None,
    Effect(Box<dyn ErasedEffectCommand<Msg>>),
    Send(Box<dyn ErasedSendCommand>),
    Notify(Box<dyn ErasedNotificationCommand>),
    Request(Box<dyn ErasedRequestCommand<Msg>>),
    Reply(Box<dyn ErasedReplyCommand>),
    After { delay: Duration, msg: Msg },
    Batch(Vec<Cmd<Msg>>),
}

impl<Msg> Cmd<Msg> {
    /// Requests no finite work.
    pub fn none() -> Self {
        Self(CmdKind::None)
    }

    /// Combines a typed effect intent with its pure message mapper.
    ///
    /// Live execution passes `effect` to the registered [`EffectAdapter<E>`].
    /// Controlled execution exposes it through
    /// [`ControlledRuntime::next_effect`] and invokes `map` when
    /// [`ControlledRuntime::complete`] supplies an [`EffectEvent`].
    pub fn effect<E, Map>(effect: E, map: Map) -> Self
    where
        Msg: Send + 'static,
        E: Effect,
        Map: FnOnce(EffectEvent<E::Output, E::Error>) -> Msg + Send + 'static,
    {
        Self(CmdKind::Effect(Box::new(Perform { effect, map })))
    }

    /// Requests one-way delivery to another Component.
    ///
    /// This is cross-Component effect intent, not a direct method call. The
    /// target's transition occurs later through normal message delivery.
    /// Delivery failure is diagnostic-only in this provisional lower-level
    /// shape. Prefer [`Cmd::notify`] for a provider-neutral one-way protocol and
    /// [`Cmd::request`] when the sender needs a typed terminal outcome.
    pub fn send<C>(target: ComponentRef<C>, msg: C::Msg) -> Self
    where
        C: Component,
    {
        Self(CmdKind::Send(Box::new(SendTo { target, msg })))
    }

    /// Requests one-way delivery through a named provider-neutral [`Port`].
    ///
    /// The sender depends on `P` and notification `N`, not the bound provider's
    /// private [`Component::Msg`] enum. The runtime converts `N` through
    /// [`Notification::into_inbound`] and routes it using the exact Port binding
    /// declared by [`ProgramBuilder::bind_port`]. No provider transition occurs
    /// while this command is constructed.
    pub fn notify<P, N>(port: Port<P>, notification: N) -> Self
    where
        P: Protocol,
        N: Notification<P>,
    {
        Self(CmdKind::Notify(Box::new(Notify { port, notification })))
    }

    /// Sends a correlated request through a named provider-neutral [`Port`].
    ///
    /// Interpreting the command creates a one-shot [`ReplyTo<R::Reply>`],
    /// converts the request through [`Request::into_inbound`], and later invokes
    /// `map` with exactly one terminal [`RequestOutcome`]. The mapper is
    /// synchronous, pure application logic and may capture a domain correlation
    /// key. The Component never waits for the reply; the mapped message arrives
    /// through its ordinary transition path.
    ///
    /// This sketch intentionally does not yet choose a default deadline or
    /// cancellation policy for requests.
    pub fn request<P, R, Map>(port: Port<P>, request: R, map: Map) -> Self
    where
        Msg: Send + 'static,
        P: Protocol,
        R: Request<P>,
        Map: FnOnce(RequestOutcome<R::Reply>) -> Msg + Send + 'static,
    {
        Self(CmdKind::Request(Box::new(RequestCommand {
            port,
            request,
            map,
        })))
    }

    /// Emits a typed reply through an [`Incoming`] request's opaque token.
    ///
    /// Consuming the token prevents a provider transition from intentionally
    /// issuing two replies from the same authority. This command is inert data;
    /// it does not touch a channel, wake a requester, or mutate runtime state
    /// until interpreted after the provider transition commits.
    pub fn reply<ReplyValue>(reply_to: ReplyTo<ReplyValue>, reply: ReplyValue) -> Self
    where
        ReplyValue: Send + 'static,
    {
        Self(CmdKind::Reply(Box::new(Reply { reply_to, reply })))
    }

    /// Requests delivery of `msg` after a runtime-controlled duration.
    ///
    /// Live execution uses real time. Controlled execution uses logical time, so
    /// tests advance it without wall-clock sleeping.
    pub fn after(delay: Duration, msg: Msg) -> Self {
        Self(CmdKind::After { delay, msg })
    }

    /// Groups commands emitted by one transition.
    ///
    /// Grouping does not promise effect completion order. If application logic
    /// requires sequencing, that dependency needs an explicit command contract
    /// or a later message transition.
    pub fn batch(cmds: impl IntoIterator<Item = Self>) -> Self {
        Self(CmdKind::Batch(cmds.into_iter().collect()))
    }

    /// Returns whether this value requests no work.
    pub fn is_none(&self) -> bool {
        matches!(&self.0, CmdKind::None)
    }

    /// Finds the first concrete effect intent of type `E`, including inside a
    /// batch.
    ///
    /// This inspection hook is intended for direct transition tests; it does not
    /// execute the effect or compare mapper identity.
    pub fn effect_intent<E: Effect>(&self) -> Option<&E> {
        match &self.0 {
            CmdKind::Effect(command) => command.intent().downcast_ref(),
            CmdKind::Batch(commands) => commands.iter().find_map(Self::effect_intent::<E>),
            CmdKind::None
            | CmdKind::Send(_)
            | CmdKind::Notify(_)
            | CmdKind::Request(_)
            | CmdKind::Reply(_)
            | CmdKind::After { .. } => None,
        }
    }

    /// Collects notification intents of concrete type `N`, including inside a
    /// batch, paired with the exact named Port each notification targets.
    ///
    /// This is a direct-transition testing hook. It neither converts the value
    /// to [`Protocol::Inbound`] nor delivers it to a provider.
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
            CmdKind::Notify(command) => {
                if let Some(notification) = command.notification().downcast_ref() {
                    intents.push((command.port(), notification));
                }
            }
            CmdKind::Batch(commands) => {
                for command in commands {
                    command.collect_notification_intents(intents);
                }
            }
            CmdKind::None
            | CmdKind::Effect(_)
            | CmdKind::Send(_)
            | CmdKind::Request(_)
            | CmdKind::Reply(_)
            | CmdKind::After { .. } => {}
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
            CmdKind::Request(command) => {
                if let Some(request) = command.request().downcast_ref() {
                    intents.push((command.port(), request));
                }
            }
            CmdKind::Batch(commands) => {
                for command in commands {
                    command.collect_request_intents(intents);
                }
            }
            CmdKind::None
            | CmdKind::Effect(_)
            | CmdKind::Send(_)
            | CmdKind::Notify(_)
            | CmdKind::Reply(_)
            | CmdKind::After { .. } => {}
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
            CmdKind::Reply(command) => {
                if let Some(reply) = command.reply().downcast_ref() {
                    intents.push(reply);
                }
            }
            CmdKind::Batch(commands) => {
                for command in commands {
                    command.collect_reply_intents(intents);
                }
            }
            CmdKind::None
            | CmdKind::Effect(_)
            | CmdKind::Send(_)
            | CmdKind::Notify(_)
            | CmdKind::Request(_)
            | CmdKind::After { .. } => {}
        }
    }

    /// Returns the concrete type name for a top-level effect command.
    ///
    /// This is diagnostic metadata, not a stable serialization identifier. A
    /// batch returns `None`; use typed inspection for behavioral tests.
    pub fn effect_type_name(&self) -> Option<&'static str> {
        match &self.0 {
            CmdKind::Effect(command) => Some(command.intent_type_name()),
            CmdKind::None
            | CmdKind::Send(_)
            | CmdKind::Notify(_)
            | CmdKind::Request(_)
            | CmdKind::Reply(_)
            | CmdKind::After { .. }
            | CmdKind::Batch(_) => None,
        }
    }
}

trait ErasedSubscription<Msg>: Send {
    fn source(&self) -> &dyn Any;
    fn source_type_name(&self) -> &'static str;
    fn mapper_type_name(&self) -> &'static str;
}

struct MappedSource<S, Map> {
    source: S,
    map: Map,
}

impl<Msg, S, Map> ErasedSubscription<Msg> for MappedSource<S, Map>
where
    Msg: Send + 'static,
    S: Source,
    Map: Fn(SourceEvent<S::Item, S::Error>) -> Msg + Send + Sync + 'static,
{
    fn source(&self) -> &dyn Any {
        &self.source
    }

    fn source_type_name(&self) -> &'static str {
        std::any::type_name::<S>()
    }

    fn mapper_type_name(&self) -> &'static str {
        std::any::type_name::<Map>()
    }
}

/// One declarative request for an ongoing source of messages.
///
/// A subscription's logical identity is its owning [`ComponentId`] plus
/// [`SubscriptionId`]. The `Source` value is comparable configuration:
/// unchanged configuration remains active, while changed configuration is
/// replaced or reconfigured. The mapper converts repeated [`SourceEvent`] values
/// to messages and is deliberately excluded from reconciliation identity.
pub struct Subscription<Msg> {
    id: SubscriptionId,
    source: Box<dyn ErasedSubscription<Msg>>,
}

impl<Msg> Subscription<Msg> {
    /// Declares a typed source and its pure event-to-message mapping.
    ///
    /// Constructing this value starts no task and touches no external resource.
    /// `Map` is called repeatedly for the lifetime of an active source, so it is
    /// `Fn` rather than the one-shot `FnOnce` accepted by [`Cmd::effect`].
    pub fn source<S, Map>(id: SubscriptionId, source: S, map: Map) -> Self
    where
        Msg: Send + 'static,
        S: Source,
        Map: Fn(SourceEvent<S::Item, S::Error>) -> Msg + Send + Sync + 'static,
    {
        Self {
            id,
            source: Box::new(MappedSource { source, map }),
        }
    }

    /// Returns the stable key used to reconcile this subscription.
    pub fn id(&self) -> &SubscriptionId {
        &self.id
    }

    /// Returns the concrete source descriptor when it has type `S`.
    ///
    /// Direct Component tests use this to assert desired resource configuration
    /// without starting a live adapter.
    pub fn source_spec<S: Source>(&self) -> Option<&S> {
        self.source.source().downcast_ref()
    }

    /// Returns diagnostic Rust type metadata for the source descriptor.
    ///
    /// The returned name is not a stable protocol or serialization identifier.
    pub fn source_type_name(&self) -> &'static str {
        self.source.source_type_name()
    }
}

/// Complete desired subscription set for one Component model.
///
/// The runtime compares this set with the active set after a committed
/// transition. Omitting a previously desired identity requests cancellation.
pub struct Subscriptions<Msg>(Vec<Subscription<Msg>>);

impl<Msg> Subscriptions<Msg> {
    /// Declares no ongoing sources.
    pub fn none() -> Self {
        Self(Vec::new())
    }

    /// Creates a desired set containing exactly one subscription.
    pub fn one(subscription: Subscription<Msg>) -> Self {
        Self(vec![subscription])
    }

    /// Iterates over the desired subscriptions without executing them.
    pub fn iter(&self) -> impl Iterator<Item = &Subscription<Msg>> {
        self.0.iter()
    }

    /// Finds source configuration `S` for one stable subscription identity.
    ///
    /// This is primarily an inspection helper for direct Component tests.
    pub fn find<S: Source>(&self, id: &SubscriptionId) -> Option<&S> {
        self.0
            .iter()
            .find(|subscription| subscription.id() == id)
            .and_then(Subscription::source_spec)
    }
}

impl<Msg> From<Vec<Subscription<Msg>>> for Subscriptions<Msg> {
    fn from(value: Vec<Subscription<Msg>>) -> Self {
        Self(value)
    }
}

/// Logical inbound channel of `T` values.
///
/// This descriptor is safe to store in a Component. The unique Tokio
/// `mpsc::Receiver<T>` exists only in live assembly through
/// [`LiveRuntimeBuilder::bind_mpsc`]. Controlled tests bind the same logical
/// source through [`ControlledRuntimeBuilder::control_mpsc`]. Channel closure is
/// delivered as [`SourceEvent::Ended`].
pub struct MpscSource<T> {
    binding: Arc<str>,
    marker: PhantomData<fn() -> T>,
}

impl<T> MpscSource<T> {
    /// Creates a reusable logical binding name for an external receiver.
    pub fn named(binding: impl Into<Arc<str>>) -> Self {
        Self {
            binding: binding.into(),
            marker: PhantomData,
        }
    }
}

impl<T> Clone for MpscSource<T> {
    fn clone(&self) -> Self {
        Self {
            binding: self.binding.clone(),
            marker: PhantomData,
        }
    }
}

impl<T> PartialEq for MpscSource<T> {
    fn eq(&self, other: &Self) -> bool {
        self.binding == other.binding
    }
}

impl<T> Eq for MpscSource<T> {}

impl<T> fmt::Debug for MpscSource<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("MpscSource")
            .field(&self.binding)
            .finish()
    }
}

impl<T: Send + 'static> Source for MpscSource<T> {
    type Item = T;
    type Error = std::convert::Infallible;
}

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
    /// Protocol failure produced while decoding a chunk.
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
}

/// Source composition that applies a pure stateful [`Decoder`] to another
/// [`Source`].
///
/// Live execution binds the underlying source to a world-facing adapter.
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

/// Terminal failure from either layer of a [`Framed`] source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FramedFailure<SourceError, DecodeError> {
    /// The underlying world-facing source failed.
    Source(SourceError),
    /// The pure decoder rejected input from an otherwise active source.
    Decode(DecodeError),
}

impl<S, D> Source for Framed<S, D>
where
    S: Source<Item = D::Chunk>,
    D: Decoder,
{
    type Item = D::Frame;
    type Error = FramedFailure<S::Error, D::Error>;
}

/// Named, immutable logical dependency on a provider-neutral [`Protocol`].
///
/// A Component may store this value and use it with [`Cmd::notify`] or
/// [`Cmd::request`]. It contains no provider reference, channel, runtime handle,
/// or lookup capability. [`ProgramBuilder::bind_port`] selects the provider
/// during program assembly, allowing the same Component definition to receive
/// real, mock, or controlled providers without changing its transition logic.
pub struct Port<P: Protocol> {
    id: PortId,
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
/// A Component may retain this value and pass it to [`Cmd::send`] when tight
/// coupling to another Component's complete message API is intentional. Normal
/// reusable dependencies should prefer [`Port`], which exposes only a stable
/// provider-neutral protocol. A Component reference exposes no model access and
/// does not itself send messages, so transitions receive no runtime capability.
/// This address is independent of mailbox or task topology.
pub struct ComponentRef<C: Component> {
    id: ComponentId,
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
pub struct Program;

impl Program {
    /// Begins assembling a program blueprint.
    pub fn builder() -> ProgramBuilder {
        ProgramBuilder {
            next_component_id: 0,
            next_port_id: 0,
        }
    }
}

/// Mutable builder used to register Components and obtain typed references.
///
/// Duplicate Component, Port, and binding validation is intentionally not
/// specified by the sketch; a real builder is expected to reject ambiguous
/// program graphs at build time.
pub struct ProgramBuilder {
    next_component_id: u64,
    next_port_id: u64,
}

impl ProgramBuilder {
    /// Adds a Component instance and returns its typed logical address.
    ///
    /// Registration order is assembly detail, not an application execution
    /// order. The Component value may hold only logical configuration and typed
    /// references—not live runtime resources.
    pub fn component<C>(&mut self, id: ComponentId, component: C) -> ComponentRef<C>
    where
        C: Component,
    {
        let _ = component;
        self.next_component_id += 1;
        ComponentRef {
            id,
            marker: PhantomData,
        }
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
        self.next_port_id += 1;
        Port {
            id,
            marker: PhantomData,
        }
    }

    /// Binds one exact named Port to a provider Component.
    ///
    /// `map` is a pure assembly-time route from the provider-neutral
    /// [`Protocol::Inbound`] value to the selected Component's private message
    /// type. A different binding can map another named `Port<P>` to another
    /// provider—or distinguish two instances routed to the same provider.
    ///
    /// This compiler-only sketch does not yet choose duplicate, missing, or
    /// cyclic binding diagnostics. A real [`ProgramBuilder::build`] must validate
    /// the assembled graph rather than falling back to a global binding by Rust
    /// `TypeId`.
    pub fn bind_port<P, C, Map>(&mut self, port: &Port<P>, provider: &ComponentRef<C>, map: Map)
    where
        P: Protocol,
        C: Component,
        Map: Fn(P::Inbound) -> C::Msg + Send + Sync + 'static,
    {
        let _ = (port, provider, map);
    }

    /// Finishes the topology-neutral program blueprint.
    pub fn build(self) -> Program {
        let _ = self;
        Program
    }
}

/// Live interpreter for one concrete [`Effect`] type.
///
/// This is an explicitly impure boundary. The runtime owns and supervises the
/// returned future, then passes its result through the pure mapper stored in the
/// originating [`Cmd`]. Controlled execution does not call this adapter.
pub trait EffectAdapter<E: Effect>: Send + Sync + 'static {
    /// Executes one typed intent and returns its typed success or failure.
    fn execute(&self, effect: E) -> BoxFuture<Result<E::Output, E::Error>>;
}

/// Live interpreter for one concrete world-facing [`Source`] type.
///
/// The returned future belongs to the runtime's structured-concurrency scope.
/// It may emit many items through [`SourceSink::emit`], then should terminate
/// explicitly with [`SourceSink::fail`] or [`SourceSink::end`]. If the sink
/// returns [`AdapterStopped`], the adapter should promptly release its resources
/// and return. The meaning of returning without a terminal sink call remains an
/// open API question in this sketch.
pub trait SourceAdapter<S: Source>: Send + Sync + 'static {
    /// Runs one active instance of the source descriptor.
    fn run(&self, source: S, sink: SourceSink<S>) -> BoxFuture<()>;
}

/// Runtime-owned output channel supplied to a live [`SourceAdapter`].
///
/// The sink translates adapter activity into [`SourceEvent`] values for the
/// subscription mapper. It offers no path to Component state.
pub struct SourceSink<S: Source> {
    marker: PhantomData<fn() -> S>,
}

impl<S: Source> SourceSink<S> {
    /// Emits one item while leaving the source active.
    ///
    /// `AdapterStopped` means the owning subscription or runtime scope no longer
    /// accepts events.
    pub async fn emit(&self, item: S::Item) -> Result<(), AdapterStopped> {
        let _ = item;
        Err(AdapterStopped)
    }

    /// Emits a terminal source failure.
    pub async fn fail(&self, error: S::Error) -> Result<(), AdapterStopped> {
        let _ = error;
        Err(AdapterStopped)
    }

    /// Emits normal terminal completion.
    pub async fn end(&self) -> Result<(), AdapterStopped> {
        Err(AdapterStopped)
    }
}

/// Signal that runtime ownership of a live adapter has ended.
#[derive(Clone, Copy, Debug)]
pub struct AdapterStopped;

impl fmt::Display for AdapterStopped {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the runtime stopped the adapter")
    }
}

impl Error for AdapterStopped {}

/// Placeholder error returned by unimplemented runtime methods in this sketch.
///
/// It exists only to make consumer control flow compile and is not a proposed
/// production error taxonomy.
#[derive(Clone, Debug)]
pub struct RuntimeError(&'static str);

impl RuntimeError {
    fn sketch() -> Self {
        Self("API sketch has no runtime implementation")
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for RuntimeError {}

/// Binds a topology-neutral [`Program`] to live Tokio world interpreters.
///
/// Binding is assembly-time work: Components still see only typed effect and
/// source descriptors. Missing or duplicate bindings should make
/// [`LiveRuntimeBuilder::build`] fail in a real implementation; the exact error
/// policy is not designed here.
pub struct LiveRuntimeBuilder {
    program: Program,
}

/// Fully assembled live program that will use real Tokio scheduling, time, and
/// world adapters.
///
/// Live execution preserves per-Component serialization and causality, but does
/// not promise deterministic order between independent events.
pub struct LiveRuntime;

impl LiveRuntime {
    /// Starts live assembly for a topology-neutral program blueprint.
    pub fn builder(program: Program) -> LiveRuntimeBuilder {
        LiveRuntimeBuilder { program }
    }

    /// Creates external ingress for one typed Component.
    ///
    /// Unlike [`ComponentRef`], a handle is a live runtime capability and must
    /// never be placed in a Component or passed to its transition.
    pub fn handle<C: Component>(
        &self,
        component: &ComponentRef<C>,
    ) -> Result<ComponentHandle<C>, RuntimeError> {
        Ok(ComponentHandle {
            component: component.clone(),
        })
    }

    /// Starts the assembled program in a runtime-owned structured scope.
    ///
    /// The returned [`RuntimeTask`] is the ownership handle used to shut down and
    /// account for all Samara-authorized work.
    pub fn spawn(self) -> RuntimeTask {
        RuntimeTask
    }
}

impl LiveRuntimeBuilder {
    /// Binds a logical [`MpscSource`] to one concrete Tokio receiver.
    ///
    /// The receiver is unique live-world state and therefore stays out of the
    /// Component and [`Program`]. Closing it produces [`SourceEvent::Ended`].
    pub fn bind_mpsc<T: Send + 'static>(
        self,
        source: MpscSource<T>,
        receiver: tokio::sync::mpsc::Receiver<T>,
    ) -> Self {
        let _ = (source, receiver);
        self
    }

    /// Registers the live interpreter for effect type `E`.
    pub fn bind_effect<E, Adapter>(self, adapter: Adapter) -> Self
    where
        E: Effect,
        Adapter: EffectAdapter<E>,
    {
        let _ = adapter;
        self
    }

    /// Registers the live interpreter for world-facing source type `S`.
    ///
    /// Pure source composition such as [`Framed`] is evaluated above this
    /// binding, so adapters operate on raw world events rather than application
    /// messages.
    pub fn bind_source<S, Adapter>(self, adapter: Adapter) -> Self
    where
        S: Source,
        Adapter: SourceAdapter<S>,
    {
        let _ = adapter;
        self
    }

    /// Validates bindings and finishes live assembly.
    pub fn build(self) -> Result<LiveRuntime, RuntimeError> {
        let _ = self.program;
        Ok(LiveRuntime)
    }
}

/// Live external sender for one typed Component.
///
/// Application Components normally communicate through [`Cmd::notify`] and
/// [`Cmd::request`], or through lower-level [`Cmd::send`] when tight coupling is
/// intentional. This capability is for surrounding Tokio code at the Samara
/// program boundary.
pub struct ComponentHandle<C: Component> {
    component: ComponentRef<C>,
}

impl<C: Component> ComponentHandle<C> {
    /// Submits a message to the live runtime.
    ///
    /// In this provisional shape, successful return means accepted for delivery,
    /// not that the target transition has completed.
    pub async fn send(&self, msg: C::Msg) -> Result<(), RuntimeError> {
        let _ = (&self.component, msg);
        Err(RuntimeError::sketch())
    }
}

/// Ownership handle for a running live Samara program.
///
/// Dropping or shutting down this scope must not leave detached commands,
/// subscriptions, or adapter work.
pub struct RuntimeTask;

impl RuntimeTask {
    /// Ends the program using the requested provisional shutdown policy and
    /// returns work-accounting evidence.
    pub async fn shutdown(self, mode: Shutdown) -> Result<ShutdownReport, RuntimeError> {
        let _ = (self, mode);
        Err(RuntimeError::sketch())
    }
}

/// Provisional policy for runtime-owned work during shutdown.
///
/// Exact drain-versus-cancel behavior for ongoing subscriptions, recurring
/// timers, and newly emitted messages remains a deferred design decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shutdown {
    /// Attempt to finish eligible in-flight work before ending the scope.
    Drain,
    /// Cancel runtime-owned work and end the scope promptly.
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
}

impl ShutdownReport {
    /// Returns whether scope closure left no owned work behind.
    pub fn is_clean(&self) -> bool {
        self.remaining == 0
    }
}

/// Declares which effect and source boundaries the controlled world may script.
///
/// Controlled assembly never falls back to a live adapter. Missing controlled
/// behavior must fail explicitly so a test cannot accidentally touch the world.
pub struct ControlledRuntimeBuilder {
    program: Program,
}

/// Deterministic, synchronously driven execution of a [`Program`].
///
/// Components, commands, subscriptions, decoders, and logical adapter contracts
/// are identical to live execution. The difference is runtime decisions: tests
/// supply external events, effect outcomes, and logical-time progression.
pub struct ControlledRuntime;

impl ControlledRuntime {
    /// Starts controlled assembly for a topology-neutral program blueprint.
    pub fn builder(program: Program) -> ControlledRuntimeBuilder {
        ControlledRuntimeBuilder { program }
    }

    /// Enqueues a typed message at a Component boundary.
    ///
    /// The transition runs only when a drive method such as
    /// [`ControlledRuntime::run_until_idle`] is called.
    pub fn send<C: Component>(
        &mut self,
        component: &ComponentRef<C>,
        msg: C::Msg,
    ) -> Result<(), RuntimeError> {
        let _ = (component, msg);
        Err(RuntimeError::sketch())
    }

    /// Emits one item through a controlled first-party [`MpscSource`].
    pub fn emit_mpsc<T: Send + 'static>(
        &mut self,
        source: &MpscSource<T>,
        item: T,
    ) -> Result<(), RuntimeError> {
        let _ = (source, item);
        Err(RuntimeError::sketch())
    }

    /// Ends a controlled first-party [`MpscSource`] normally.
    ///
    /// Its subscription mapper receives [`SourceEvent::Ended`].
    pub fn close_mpsc<T: Send + 'static>(
        &mut self,
        source: &MpscSource<T>,
    ) -> Result<(), RuntimeError> {
        let _ = source;
        Err(RuntimeError::sketch())
    }

    /// Injects an event at a typed source layer of an active subscription.
    ///
    /// For `Framed<TcpBytes, Decoder>`, controlled tests select the underlying
    /// `TcpBytes` layer and inject raw chunks. Samara then runs the same decoder
    /// used by live execution before invoking the subscription mapper. Injection
    /// into an inactive identity or absent source layer is an error.
    pub fn emit_source<C, S>(
        &mut self,
        component: &ComponentRef<C>,
        id: &SubscriptionId,
        event: SourceEvent<S::Item, S::Error>,
    ) -> Result<(), RuntimeError>
    where
        C: Component,
        S: Source,
    {
        let _ = (component, id, event);
        Err(RuntimeError::sketch())
    }

    /// Returns a clone of the active source descriptor for inspection.
    ///
    /// Tests use this to verify subscription reconciliation and resource
    /// configuration without observing adapter-owned operational state.
    pub fn subscription_spec<C, S>(
        &self,
        component: &ComponentRef<C>,
        id: &SubscriptionId,
    ) -> Result<S, RuntimeError>
    where
        C: Component,
        S: Source,
    {
        let _ = (component, id);
        Err(RuntimeError::sketch())
    }

    /// Removes and returns the next pending effect of concrete type `E`.
    ///
    /// Selection follows the controlled scheduler's stable deterministic order,
    /// which is reproducibility machinery rather than a live ordering promise.
    pub fn next_effect<E: Effect>(&mut self) -> Result<PendingEffect<E>, RuntimeError> {
        Err(RuntimeError::sketch())
    }

    /// Supplies a terminal event for a previously intercepted effect.
    ///
    /// Completion invokes the command's pure mapper and enqueues the resulting
    /// message; it never executes a live [`EffectAdapter`].
    pub fn complete<E: Effect>(
        &mut self,
        pending: PendingEffect<E>,
        event: EffectEvent<E::Output, E::Error>,
    ) -> Result<(), RuntimeError> {
        let _ = (pending, event);
        Err(RuntimeError::sketch())
    }

    /// Processes immediately runnable work until the program is quiescent now.
    ///
    /// Future timers and open subscriptions remain pending and do not prevent
    /// return. This method does not advance logical time.
    pub fn run_until_idle(&mut self) -> Result<RunReport, RuntimeError> {
        Err(RuntimeError::sketch())
    }

    /// Advances logical time by `duration`, then processes newly runnable work
    /// until immediate quiescence.
    pub fn advance(&mut self, duration: Duration) -> Result<RunReport, RuntimeError> {
        let _ = duration;
        Err(RuntimeError::sketch())
    }

    /// Advances to the next scheduled logical instant and processes work there.
    ///
    /// Returns an error when no future scheduled work exists. Repeated calls are
    /// the primitive behind automatic time advancement.
    pub fn advance_to_next(&mut self) -> Result<RunReport, RuntimeError> {
        Err(RuntimeError::sketch())
    }

    /// Closes this controlled scope by cancelling all remaining owned work.
    ///
    /// Consuming the runtime prevents the harness from supplying any further
    /// controlled input. Cancellation and accounting proceed in deterministic
    /// controlled-runtime order, covering commands, subscriptions, timers, and
    /// any adapter work owned by the scope. Successful return guarantees that
    /// [`ShutdownReport::remaining`] is zero.
    ///
    /// This is lifecycle and diagnostic behavior only: it does not mean the
    /// application reached a domain-defined completion state. A test that needs
    /// a Component to observe cancellation must explicitly supply the
    /// appropriate typed cancellation outcome before calling this method.
    pub fn cancel(self) -> Result<ShutdownReport, RuntimeError> {
        let _ = self;
        Err(RuntimeError::sketch())
    }

    /// Borrows the current model while controlled execution is paused.
    ///
    /// Live execution deliberately has no equivalent shared model handle.
    pub fn state<C: Component>(
        &self,
        component: &ComponentRef<C>,
    ) -> Result<&C::Model, RuntimeError> {
        let _ = component;
        Err(RuntimeError::sketch())
    }

    /// Returns the deterministic structured semantic trace accumulated so far.
    ///
    /// Trace observation must not feed behavior back into Components. The small
    /// [`TraceEvent`] enum below is illustrative rather than exhaustive.
    pub fn trace(&self) -> &[TraceEvent] {
        &[]
    }
}

impl ControlledRuntimeBuilder {
    /// Allows tests to script one logical first-party [`MpscSource`].
    pub fn control_mpsc<T: Send + 'static>(self, source: MpscSource<T>) -> Self {
        let _ = source;
        self
    }

    /// Allows tests to intercept and complete effect type `E`.
    pub fn control_effect<E: Effect>(self) -> Self {
        self
    }

    /// Allows tests to inject events for world-facing source type `S`.
    ///
    /// For a composed [`Framed`] source, register and inject the underlying
    /// source type so the decoder remains part of the program under test.
    pub fn control_source<S: Source>(self) -> Self {
        self
    }

    /// Validates controlled bindings and creates a paused deterministic runtime.
    pub fn build(self) -> Result<ControlledRuntime, RuntimeError> {
        let _ = self.program;
        Ok(ControlledRuntime)
    }
}

/// Typed effect intercepted before any live world interaction occurs.
pub struct PendingEffect<E: Effect> {
    /// Original effect intent emitted by the Component transition.
    pub intent: E,
    id: u64,
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

/// Illustrative structured semantic trace emitted out of band by the runtime.
///
/// Production tracing will need more event kinds and explicit causal identifiers;
/// this sketch only establishes that trace data is structured and observable in
/// controlled tests without becoming Component input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceEvent {
    /// A Component committed one message transition.
    Transition {
        /// Component that owned the transition.
        component: ComponentId,
        /// Diagnostic Rust type name of the delivered message.
        message_type: &'static str,
    },
    /// A transition emitted an interceptable effect intent.
    EffectRequested {
        /// Component that requested the effect.
        component: ComponentId,
        /// Diagnostic Rust type name of the effect intent.
        effect_type: &'static str,
    },
    /// Reconciliation started a newly desired subscription.
    SubscriptionStarted {
        /// Component that owns the subscription.
        component: ComponentId,
        /// Stable subscription identity within that Component.
        subscription: SubscriptionId,
        /// Diagnostic Rust type name of its source descriptor.
        source_type: &'static str,
    },
    /// Reconciliation stopped a removed, replaced, failed, or ended subscription.
    SubscriptionStopped {
        /// Component that owned the subscription.
        component: ComponentId,
        /// Stable subscription identity within that Component.
        subscription: SubscriptionId,
    },
}

#[cfg(test)]
mod protocol_macro_tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    struct Snapshot(u8);

    protocol! {
        type MacroProtocol => enum MacroInbound {
            SnapshotRequest -> Snapshot,
            Observation(u8),
            Refresh,
            Lookup(u16) -> Snapshot,
        }
    }

    protocol! {
        type UnitReplyProtocol => enum UnitReplyInbound {
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
        let inbound = <Observation as Notification<MacroProtocol>>::into_inbound(Observation(7));

        match inbound {
            MacroInbound::Observation(observed) => assert_eq!(observed, 7),
            _ => panic!("notification became a different operation"),
        }
    }

    #[test]
    fn generated_unit_notification_is_flattened() {
        let inbound = <Refresh as Notification<MacroProtocol>>::into_inbound(Refresh);

        assert!(matches!(inbound, MacroInbound::Refresh));
    }

    #[test]
    fn generated_unit_request_preserves_typed_reply_association() {
        fn assert_reply_type<R>()
        where
            R: Request<MacroProtocol, Reply = Snapshot>,
        {
        }

        assert_reply_type::<SnapshotRequest>();
        let inbound = <SnapshotRequest as Request<MacroProtocol>>::into_inbound(
            SnapshotRequest,
            ReplyTo::<Snapshot>::sketch(41),
        );

        match inbound {
            MacroInbound::SnapshotRequest(incoming) => {
                let SnapshotRequest = incoming.request;
                assert_eq!(incoming.reply_to.correlation, 41);
                let _: ReplyTo<Snapshot> = incoming.reply_to;
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
        let inbound = <Lookup as Request<MacroProtocol>>::into_inbound(
            Lookup(17),
            ReplyTo::<Snapshot>::sketch(42),
        );

        match inbound {
            MacroInbound::Lookup(incoming) => {
                assert_eq!(incoming.request.0, 17);
                assert_eq!(incoming.reply_to.correlation, 42);
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
