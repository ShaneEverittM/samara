#![allow(dead_code)]
#![warn(missing_docs)]

//! Build message-driven applications that run on Tokio or under test control.
//!
//! A [`Component`] owns a model. Its `update` method handles one message at a time,
//! changes the model, and returns a [`Command`] describing what to do next.
//! [`Subscription`]s describe inputs the component wants to keep receiving.
//!
//! Use [`LiveRuntime`] to run the application with I/O. Use [`ControlledRuntime`]
//! to supply inputs and results yourself, and advance time without waiting.
//! Both run the same component code.
//!
//! # A component and a test
//!
//! ```
//! use samara::prelude::*;
//!
//! struct Counter;
//! enum Message { Add(u64) }
//!
//! impl Component for Counter {
//!     type Model = u64;
//!     type Message = Message;
//!
//!     fn init(&self) -> Init<u64, Message> {
//!         Init::new(0)
//!     }
//!
//!     fn update(&self, count: &mut u64, message: Message) -> Command<Message> {
//!         match message {
//!             Message::Add(amount) => *count += amount,
//!         }
//!         Command::none()
//!     }
//! }
//!
//! let mut builder = Program::builder();
//! let counter = builder.component(ComponentId::new("counter"), Counter);
//! let mut runtime = ControlledRuntime::builder(builder.build()?).build()?;
//! runtime.send(&counter, Message::Add(3))?;
//! runtime.run_until_idle()?;
//! assert_eq!(*runtime.state(&counter)?, 3);
//! assert!(runtime.cancel()?.is_clean());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Where to go next
//!
//! - [`StdinLines`]: connect a component to input, with live and controlled examples.
//! - [`Command`]: request work, send messages, and schedule timers.
//! - [`HttpRequest`]: fetch a URL and handle its response.
//! - [`ProgramBuilder::bind_port`]: connect components through a shared protocol.
//! - [`EffectDriver`] and [`SourceDriver`]: implement your own I/O.
//! - [`RuntimeTask`]: shut down a running application and wait for cleanup.
//!
//! Keep `init`, `update`, `subscriptions`, message conversions, and decoders free
//! of I/O, blocking, locks, and clock reads. Describe work with commands and
//! subscriptions; drivers perform the I/O. Each component's updates run without
//! overlapping. Independent live events can arrive in either order; controlled
//! runs produce the same trace for the same inputs.

use std::{
    any::{Any, TypeId},
    error::Error,
    fmt,
    future::Future,
    marker::PhantomData,
    net::SocketAddr,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

mod component_kernel;
mod controlled_runtime;
mod declarative_work;
mod http_redirect;
mod live_runtime;

use component_kernel::{ComponentKernel, ErasedComponentKernel};
use controlled_runtime::ControlledCore;

/// HTTP response status used by [`HttpResponse::status`].
///
/// ```
/// use samara::prelude::*;
///
/// let status = StatusCode::OK;
/// assert!(status.is_success());
/// assert_eq!(status.as_u16(), 200);
/// ```
pub use http::StatusCode;

/// Common types and traits for writing components and building runtimes.
///
/// ```
/// use samara::prelude::*;
/// let command: Command<()> = Command::none();
/// assert!(command.is_none());
/// ```
///
/// Formatting macros are used as `samara::println!`, `samara::eprintln!`, etc.,
/// to distinguish them from Rust's immediate I/O macros.
pub mod prelude {
    pub use crate::{
        BoxFuture, CancelReason, Command, Component, ComponentHandle, ComponentId, ComponentRef,
        ControlledRuntime, Decoder, DriverStopped, EffectCapability, EffectDescriptor,
        EffectDriver, EffectInvocation, EffectOutcome, EffectOutcomeKind, Framed, FramedError,
        FramedLayer, HttpError, HttpErrorKind, HttpJsonError, HttpRequest, HttpResponse,
        HttpResponseError, HttpStatusError, Init, LiveRuntime, LogicalTime, Notification,
        PendingEffect, PendingWork, Port, PortHandle, PortId, PrintStderr, PrintStdout, Program,
        ProgramBuildError, ProgramBuilder, Protocol, ReplyTo, Request, RequestError,
        RequestInvocation, RequestOutcome, RequestOutcomeKind, RunReport, RuntimeError,
        RuntimeTask, Shutdown, ShutdownReport, SourceCapability, SourceDescriptor, SourceDriver,
        SourceEvent, SourceEventKind, SourceSink, StatusCode, StdinError, StdinErrorKind,
        StdinLines, StreamDescriptor, Subscription, SubscriptionAction, SubscriptionId,
        Subscriptions, TcpBytes, TcpError, TcpErrorKind, TraceCommandKind, TraceEvent, TraceId,
        TraceRecord, protocol,
    };
}

/// Creates a Command to write formatted text to standard output.
///
/// Return it from `update`, or include it in a [`Command::batch`], to request the
/// write. Construction does no I/O. Writing schedules no response message and
/// ignores I/O errors. Bind output with [`LiveRuntimeBuilder::bind_stdio`].
///
/// ```
/// use samara::{Command, PrintStdout, Program};
/// let mut program = Program::builder();
/// let output = program.effect::<PrintStdout>();
/// let command: Command<()> = samara::print!(&output, "count: {}", 3);
/// assert_eq!(command.effect_intent::<PrintStdout>().unwrap().as_str(), "count: 3");
/// ```
#[macro_export]
macro_rules! print {
    ($capability:expr) => {
        $crate::Command::effect_discarding_outcome(
            $capability,
            $crate::PrintStdout::text(::std::string::String::new()),
        )
    };
    ($capability:expr, $($argument:tt)+) => {
        $crate::Command::effect_discarding_outcome(
            $capability,
            $crate::PrintStdout::text(::std::format!($($argument)+))
        )
    };
}

/// Creates a Command to write formatted text to standard output and append a newline.
///
/// Return it from `update`, or include it in a [`Command::batch`], to request the
/// write. Construction does no I/O. Writing schedules no response message and
/// ignores I/O errors. Bind output with [`LiveRuntimeBuilder::bind_stdio`].
///
/// ```
/// use samara::{Command, PrintStdout, Program};
/// let mut program = Program::builder();
/// let output = program.effect::<PrintStdout>();
/// let command: Command<()> = samara::println!(&output, "count: {}", 3);
/// assert_eq!(command.effect_intent::<PrintStdout>().unwrap().as_str(), "count: 3\n");
/// ```
#[macro_export]
macro_rules! println {
    ($capability:expr) => {
        $crate::Command::effect_discarding_outcome(
            $capability,
            $crate::PrintStdout::line(::std::string::String::new()),
        )
    };
    ($capability:expr, $($argument:tt)+) => {
        $crate::Command::effect_discarding_outcome(
            $capability,
            $crate::PrintStdout::line(::std::format!($($argument)+))
        )
    };
}

/// Creates a Command to write formatted text to standard error.
///
/// Return it from `update`, or include it in a [`Command::batch`], to request the
/// write. Construction does no I/O. Writing schedules no response message and
/// ignores I/O errors. Bind output with [`LiveRuntimeBuilder::bind_stdio`].
///
/// ```
/// use samara::{Command, PrintStderr, Program};
/// let mut program = Program::builder();
/// let output = program.effect::<PrintStderr>();
/// let command: Command<()> = samara::eprint!(&output, "count: {}", 3);
/// assert_eq!(command.effect_intent::<PrintStderr>().unwrap().as_str(), "count: 3");
/// ```
#[macro_export]
macro_rules! eprint {
    ($capability:expr) => {
        $crate::Command::effect_discarding_outcome(
            $capability,
            $crate::PrintStderr::text(::std::string::String::new()),
        )
    };
    ($capability:expr, $($argument:tt)+) => {
        $crate::Command::effect_discarding_outcome(
            $capability,
            $crate::PrintStderr::text(::std::format!($($argument)+))
        )
    };
}

/// Creates a Command to write formatted text to standard error and append a newline.
///
/// Return it from `update`, or include it in a [`Command::batch`], to request the
/// write. Construction does no I/O. Writing schedules no response message and
/// ignores I/O errors. Bind output with [`LiveRuntimeBuilder::bind_stdio`].
///
/// ```
/// use samara::{Command, PrintStderr, Program};
/// let mut program = Program::builder();
/// let output = program.effect::<PrintStderr>();
/// let command: Command<()> = samara::eprintln!(&output, "count: {}", 3);
/// assert_eq!(command.effect_intent::<PrintStderr>().unwrap().as_str(), "count: 3\n");
/// ```
#[macro_export]
macro_rules! eprintln {
    ($capability:expr) => {
        $crate::Command::effect_discarding_outcome(
            $capability,
            $crate::PrintStderr::line(::std::string::String::new()),
        )
    };
    ($capability:expr, $($argument:tt)+) => {
        $crate::Command::effect_discarding_outcome(
            $capability,
            $crate::PrintStderr::line(::std::format!($($argument)+))
        )
    };
}

/// A boxed future returned by an [`EffectDriver`] or [`SourceDriver`].
///
/// Use `Box::pin(async move { ... })`. Samara runs and cancels the future;
/// keep child work inside it rather than spawning detached tasks.
///
/// ```
/// use samara::BoxFuture;
/// let work: BoxFuture<u64> = Box::pin(async { 42 });
/// ```
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// The name of a component within a Program.
///
/// Names must be unique within that program.
///
/// ```
/// use samara::ComponentId;
/// let id = ComponentId::new("input");
/// assert_eq!(id, ComponentId::new("input"));
/// ```
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ComponentId(Arc<str>);

impl ComponentId {
    /// Creates a name from a string.
    pub fn new(value: impl Into<Arc<str>>) -> Self {
        Self(value.into())
    }
}

/// The name of a protocol connection within a Program.
///
/// A port is identified by its protocol type and name. Use different names for
/// two providers of the same protocol, such as `"primary"` and `"fallback"`.
///
/// ```
/// use samara::PortId;
/// let id = PortId::new("input");
/// assert_eq!(id, PortId::new("input"));
/// ```
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PortId(Arc<str>);

impl PortId {
    /// Creates a name from a string.
    pub fn new(value: impl Into<Arc<str>>) -> Self {
        Self(value.into())
    }
}

/// The name of a subscription within one component.
///
/// Reuse the name across calls to `subscriptions` to keep the same input active.
/// Different components can use the same name. See [`Subscription`] for when
/// Samara keeps or replaces a source.
///
/// ```
/// use samara::SubscriptionId;
/// let id = SubscriptionId::new("input");
/// assert_eq!(id, SubscriptionId::new("input"));
/// ```
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubscriptionId(Arc<str>);

impl SubscriptionId {
    /// Creates a name from a string.
    pub fn new(value: impl Into<Arc<str>>) -> Self {
        Self(value.into())
    }
}

/// Describes one I/O operation and the types of its result and error.
///
/// Store the arguments needed to perform the operation. Implement [`EffectDriver`]
/// to execute it, or supply its result in a [`ControlledRuntime`] test. Creating
/// or inspecting a descriptor must not perform I/O.
///
/// ```
/// use samara::{EffectDescriptor, Program, Command};
/// struct ReadFile { path: String }
/// impl EffectDescriptor for ReadFile {
///     type Output = Vec<u8>;
///     type Error = std::io::Error;
/// }
/// let mut program = Program::builder();
/// let files = program.effect::<ReadFile>();
/// let command: Command<_> = Command::effect_with(
///     &files, ReadFile { path: "settings.txt".into() }, |outcome| outcome,
/// );
/// assert_eq!(command.effect_intent::<ReadFile>().unwrap().path, "settings.txt");
/// ```
pub trait EffectDescriptor: Send + 'static {
    /// Value produced when the effect succeeds.
    type Output: Send + 'static;
    /// Error returned when the operation fails.
    type Error: Send + 'static;
}

/// Custom descriptors cannot override internal source composition.
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
mod source_plan_private {
    /// Unnameable outside this crate, so the hidden lowering hook can be
    /// implemented by downstream descriptors but overridden only by Samara.
    pub struct LowerToken(());

    pub(crate) const LOWER_TOKEN: LowerToken = LowerToken(());
}

/// Describes an input that can produce many events, such as a socket or file watcher.
///
/// Implement [`SourceDriver`] to read the input. Use [`Subscription`] to request
/// it from a component, and [`ControlledRuntime::emit_source`] to supply test
/// inputs. Descriptor equality determines whether a subscription needs to restart:
/// include configuration such as the address or path, but keep open handles and
/// read buffers in the driver.
///
/// ```
/// use samara::{SourceDescriptor, Program, Subscription, SubscriptionId};
/// #[derive(Clone, Debug, PartialEq)]
/// struct WatchFile { path: String }
/// impl SourceDescriptor for WatchFile {
///     type Item = Vec<u8>;
///     type Error = std::io::Error;
/// }
/// let mut program = Program::builder();
/// let files = program.source::<WatchFile>();
/// let subscription = Subscription::source_with(
///     &files, SubscriptionId::new("settings"),
///     WatchFile { path: "settings.txt".into() }, |event| event,
/// );
/// assert_eq!(subscription.id(), &SubscriptionId::new("settings"));
/// ```
///
/// Use [`StdinLines`] for terminal input, [`StreamDescriptor`] for a Tokio channel,
/// or [`TcpBytes`] for a TCP connection. Wrap a source in [`Framed`] to decode its
/// items before they reach your component.
pub trait SourceDescriptor: Clone + PartialEq + fmt::Debug + Send + Sync + 'static {
    /// Item emitted while the source remains active.
    type Item: Send + 'static;
    /// Error that stops the input.
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

    /// Internal static terminal-type discovery for closed Program assembly.
    #[doc(hidden)]
    #[allow(private_interfaces)]
    fn __samara_terminal_type_id(_: source_plan_private::LowerToken) -> TypeId {
        TypeId::of::<Self>()
    }

    /// Internal static terminal-type diagnostics for closed Program assembly.
    #[doc(hidden)]
    #[allow(private_interfaces)]
    fn __samara_terminal_type_name(_: source_plan_private::LowerToken) -> &'static str {
        std::any::type_name::<Self>()
    }

    /// Internal discovery for exact `StreamDescriptor<T>` terminal adapters.
    #[doc(hidden)]
    #[allow(private_interfaces)]
    fn __samara_terminal_stream_item_type_id(_: source_plan_private::LowerToken) -> Option<TypeId> {
        None
    }
}

mod stream_source_private {
    pub trait Sealed {}
}

/// Type-level evidence that a source descriptor lowers to
/// [`StreamDescriptor<Self::StreamItem>`].
///
/// This trait is sealed and used only by Samara's exact stream bridge APIs.
/// Applications do not implement or name it; direct [`StreamDescriptor`]
/// values and built-in compositions such as [`Framed`] satisfy it
/// automatically.
#[doc(hidden)]
pub trait StreamSourceDescriptor: SourceDescriptor + stream_source_private::Sealed {
    /// Item accepted by the terminal stream bridge before any Layers run.
    type StreamItem: Send + 'static;
}

mod stdin_source_private {
    pub trait Sealed {}
}

/// Type-level evidence that a source descriptor lowers to [`StdinLines`].
///
/// This trait is sealed and used only by Samara's exact stdin binding. An
/// application may bind either `StdinLines` directly or a built-in composition
/// such as `Framed<StdinLines, D>` without exposing the terminal descriptor
/// separately.
#[doc(hidden)]
pub trait StdinSourceDescriptor: SourceDescriptor + stdin_source_private::Sealed {}

#[derive(Clone)]
pub(crate) struct CapabilityToken {
    id: u64,
    program: Arc<()>,
}

impl CapabilityToken {
    fn belongs_to(&self, program: &Arc<()>) -> bool {
        Arc::ptr_eq(&self.program, program)
    }

    fn same_as(&self, other: &Self) -> bool {
        self.id == other.id && Arc::ptr_eq(&self.program, &other.program)
    }
}

/// Runtime-owned lowering of one composed [`SourceDescriptor`].
pub(crate) struct SourcePlan {
    terminal: Box<dyn ErasedTerminalDescriptor>,
    layers: Vec<Box<dyn ErasedSourceLayer>>,
    output_event_type: TypeId,
    valid_event_chain: bool,
    capability: Option<CapabilityToken>,
}

trait ErasedTerminalDescriptor: Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn descriptor_type_id(&self) -> TypeId;
    fn type_name(&self) -> &'static str;
    fn ended_event(&self) -> ErasedSourceEvent;
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

    fn ended_event(&self) -> ErasedSourceEvent {
        ErasedSourceEvent::typed::<S>(SourceEvent::Ended)
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
            capability: None,
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

    pub(crate) fn terminal_ended_event(&self) -> ErasedSourceEvent {
        self.terminal.ended_event()
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

    pub(crate) fn capability(&self) -> &CapabilityToken {
        self.capability
            .as_ref()
            .expect("every runtime SourcePlan comes from a declared Subscription")
    }
}

/// The result of an effect: success, failure, or cancellation.
///
/// Your command's mapper converts this value to a component message.
/// [`Shutdown::Cancel`] stops work without calling that mapper; it does not
/// produce a `Cancelled` message.
///
/// ```
/// use samara::EffectOutcome;
/// let outcome: EffectOutcome<u64, &str> = EffectOutcome::Succeeded(42);
/// let text = match outcome {
///     EffectOutcome::Succeeded(value) => value.to_string(),
///     EffectOutcome::Failed(error) => format!("failed: {error}"),
///     EffectOutcome::Cancelled(reason) => format!("cancelled: {reason:?}"),
/// };
/// assert_eq!(text, "42");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EffectOutcome<Output, EffectError> {
    /// The operation succeeded and produced this value.
    Succeeded(Output),
    /// The operation failed with this error.
    Failed(EffectError),
    /// The operation was cancelled for this reason.
    ///
    /// Runtime shutdown with [`Shutdown::Cancel`] does not send this outcome or
    /// call the effect's message mapper.
    Cancelled(CancelReason),
}

/// An item, error, or end-of-input notification from a source.
///
/// `Failed` and `Ended` stop that source. Samara does not restart it automatically
/// while the subscription remains unchanged. Removing or replacing a subscription,
/// or shutting down the runtime, sends neither event.
///
/// See [`StdinLines`] for an example that handles all three variants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceEvent<Item, SourceError> {
    /// Another item from the input.
    Item(Item),
    /// The input failed and stopped.
    Failed(SourceError),
    /// The input ended normally.
    ///
    /// Normal ending does not restart the input while its
    /// subscription remains unchanged.
    Ended,
}

/// The kind of source event recorded in a [`TraceEvent`], without its payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceEventKind {
    /// The input produced an item.
    Item,
    /// The input failed and stopped.
    Failed,
    /// The input ended normally.
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

/// Why an effect was cancelled.
///
/// These reasons come from effect-specific logic or a controlled test.
/// Stopping the runtime with [`Shutdown::Cancel`] does not send effect outcomes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CancelReason {
    /// The operation was cancelled because its scope shut down.
    Shutdown,
    /// Newer work made the operation obsolete.
    Superseded,
    /// The operation exceeded its deadline.
    Deadline,
}

/// The model and startup command returned by [`Component::init`].
///
/// ```
/// use samara::{Command, Init};
/// use std::time::Duration;
/// let init: Init<u64, ()> = Init::new(0)
///     .with_command(Command::after(Duration::from_secs(1), ()));
/// assert_eq!(init.model, 0);
/// ```
///
/// Use `Init::default()` when the model implements [`Default`].
pub struct Init<Model, Message> {
    /// The starting model.
    pub model: Model,
    /// The command to run at startup.
    pub command: Command<Message>,
}

impl<Model, Message> Init<Model, Message> {
    /// Creates an initial model with no startup work.
    pub fn new(model: Model) -> Self {
        Self {
            model,
            command: Command::none(),
        }
    }

    /// Sets the command to run when the runtime starts.
    ///
    /// Replaces any previously set command. Use [`Command::batch`] for several actions.
    pub fn with_command(mut self, command: Command<Message>) -> Self {
        self.command = command;
        self
    }
}

impl<Model, Message> Default for Init<Model, Message>
where
    Model: Default,
{
    fn default() -> Self {
        Self::new(Model::default())
    }
}

/// A part of an application with its own state and messages.
///
/// Put changing application state in `Model`. Keep configuration, capabilities,
/// and ports on the component itself. Samara owns the model and calls `update`
/// for each message, never overlapping updates to the same component.
///
/// All three methods must be deterministic and free of I/O, blocking, locks, and
/// clock reads. Return commands and subscriptions to request work.
///
/// ```
/// use samara::prelude::*;
/// struct Counter;
/// impl Component for Counter {
///     type Model = u64;
///     type Message = u64;
///     fn init(&self) -> Init<u64, u64> { Init::new(0) }
///     fn update(&self, count: &mut u64, amount: u64) -> Command<u64> {
///         *count += amount;
///         Command::none()
///     }
/// }
/// let mut model = Counter.init().model;
/// assert!(Counter.update(&mut model, 3).is_none());
/// assert_eq!(model, 3);
/// ```
///
/// See [`StdinLines`] for subscriptions and [`ProgramBuilder::bind_port`] for
/// communication between components.
///
/// <details>
/// <summary>Compile-time checks</summary>
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
///
/// </details>
pub trait Component: Send + 'static {
    /// This component's mutable application state.
    type Model: Send + 'static;
    /// Messages this component handles in `update`.
    type Message: Send + 'static;

    /// Creates the starting model and optional startup work. Called when the component is registered.
    fn init(&self) -> Init<Self::Model, Self::Message>;

    /// Handles one message, updates the model, and returns the next actions.
    ///
    /// Use [`Command::none`] when no work is needed. Do not perform I/O here.
    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message>;

    /// Returns the inputs this component wants for the current model.
    ///
    /// Called for the initial model and after each update. Samara starts new inputs,
    /// keeps unchanged ones, and stops those omitted from the returned set.
    /// The default returns no subscriptions. See [`Subscription`] for comparison rules.
    fn subscriptions(&self, _model: &Self::Model) -> Subscriptions<Self::Message> {
        Subscriptions::none()
    }
}

/// Defines the messages that can be sent through a [`Port`].
///
/// A protocol lets callers use a service without depending on its component's
/// message type. The provider implements `From<P::Message>` for its own message
/// type. Use [`protocol!`] to generate request and notification types, or define
/// them yourself. See [`ProgramBuilder::bind_port`] for a complete example.
pub trait Protocol: Send + Sync + 'static {
    /// Messages accepted by a provider of this protocol.
    type Message: Send + 'static;
}

/// A message sent through a [`Port`] without expecting a reply.
///
/// `into_message` must be a pure conversion. [`protocol!`] generates this
/// implementation for entries without `-> Reply`.
///
/// ```
/// use samara::prelude::*;
/// protocol! { type Alerts => enum AlertMessage { Changed(String), } }
/// let mut program = Program::builder();
/// let alerts = program.port::<Alerts>(PortId::new("alerts"));
/// let command: Command<()> = Command::notify(alerts, Changed("ready".into()));
/// assert_eq!(command.notification_intents::<Changed>().len(), 1);
/// ```
pub trait Notification<P: Protocol>: Send + 'static {
    /// Converts this notification to a protocol message.
    fn into_message(self) -> P::Message;
}

/// A message sent through a [`Port`] that expects a reply of type `Reply`.
///
/// `into_message` packages the request with the reply token; it must not perform
/// I/O. [`protocol!`] generates this implementation for entries with `-> Reply`.
/// Use [`Command::request_with`] to receive the reply in your component.
///
/// ```
/// use samara::prelude::*;
/// protocol! { type Catalog => enum CatalogMessage { Lookup(String) -> Option<u64>, } }
/// let mut program = Program::builder();
/// let catalog = program.port::<Catalog>(PortId::new("catalog"));
/// let command = Command::request_with(catalog, Lookup("widget".into()), |outcome| outcome);
/// assert_eq!(command.request_intents::<Lookup>().len(), 1);
/// ```
pub trait Request<P: Protocol>: Send + 'static {
    /// The value returned in response to this request.
    type Reply: Send + 'static;

    /// Packages this request and its reply token in a protocol message.
    fn into_message(self, reply_to: ReplyTo<Self::Reply>) -> P::Message;
}

/// The reply or timeout delivered to a request's caller.
///
/// Requests have no timeout unless you use [`Command::request_timeout`] or
/// [`PortHandle::request_timeout`]. A timeout leaves the provider's work running
/// and discards its later reply. Runtime cancellation does not send this value
/// to a component. `Failed` and `Cancelled` are not produced by the built-in
/// request implementation.
///
/// ```
/// use samara::RequestOutcome;
/// let outcome: RequestOutcome<u64> = RequestOutcome::TimedOut;
/// let value = match outcome {
///     RequestOutcome::Replied(value) => Some(value),
///     RequestOutcome::TimedOut | RequestOutcome::Cancelled | RequestOutcome::Failed(_) => None,
/// };
/// assert_eq!(value, None);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestOutcome<Reply> {
    /// The provider replied with this value.
    Replied(Reply),
    /// The request failed. Not produced by the built-in request implementation.
    Failed(RequestError),
    /// No reply was processed before the deadline.
    TimedOut,
    /// The request was cancelled. Not produced by the built-in request implementation.
    Cancelled,
}

/// The reason for a [`RequestOutcome::Failed`] value.
///
/// The built-in request implementation does not currently produce these errors.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RequestError {
    /// The request could not be delivered to its provider.
    DeliveryFailed,
    /// The provider accepted the request but can no longer reply.
    ReplyAbandoned,
}

/// The token a provider uses to reply to a request.
///
/// Pass it to [`Command::reply`]. It cannot be cloned or created by application
/// code, so each request can be answered at most once. Dropping it does not cancel
/// the request: the caller keeps waiting until its timeout or runtime shutdown.
/// An unanswered request without a timeout can keep [`Shutdown::Drain`] waiting.
///
/// See [`ProgramBuilder::bind_port`] for a provider that returns a reply.
///
/// <details>
/// <summary>Compile-time checks</summary>
///
/// ```compile_fail
/// #![deny(unused_must_use)]
/// use samara::ReplyTo;
///
/// fn discard(reply_to: ReplyTo<()>) {
///     reply_to;
/// }
/// ```
///
/// </details>
#[must_use = "a ReplyTo must be consumed by Command::reply"]
pub struct ReplyTo<Reply> {
    token: Arc<RequestToken>,
    marker: PhantomData<fn(Reply)>,
}

// Only runtime interpretation reads or changes this opaque correlation state.
// An expired provider token retains no continuation or runtime handle, and the
// runtime need not retain a tombstone if the provider never replies.
struct RequestToken {
    correlation: u64,
    program: Arc<()>,
    expired: AtomicBool,
}

impl RequestToken {
    fn new(correlation: u64, program: Arc<()>) -> Arc<Self> {
        Arc::new(Self {
            correlation,
            program,
            expired: AtomicBool::new(false),
        })
    }

    fn expire(&self) {
        self.expired.store(true, Ordering::Relaxed);
    }

    fn is_expired(&self) -> bool {
        self.expired.load(Ordering::Relaxed)
    }
}

impl<Reply> ReplyTo<Reply> {
    #[cfg(test)]
    fn sketch(correlation: u64) -> Self {
        Self::runtime(RequestToken::new(correlation, Arc::new(())))
    }

    fn runtime(token: Arc<RequestToken>) -> Self {
        Self {
            token,
            marker: PhantomData,
        }
    }
}

impl<Reply> fmt::Debug for ReplyTo<Reply> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("ReplyTo").field(&"<opaque>").finish()
    }
}

/// A received request and the token used to answer it.
///
/// Read `request` and return [`Command::reply`] with `reply_to`.
/// See [`ProgramBuilder::bind_port`] for a complete provider example.
pub struct RequestInvocation<P, R>
where
    P: Protocol,
    R: Request<P>,
{
    /// The request arguments.
    pub request: R,
    /// The token to pass to [`Command::reply`].
    pub reply_to: ReplyTo<R::Reply>,
    protocol: PhantomData<fn() -> P>,
}

impl<P, R> RequestInvocation<P, R>
where
    P: Protocol,
    R: Request<P>,
{
    /// Packages a request and its reply token inside [`Request::into_message`].
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

/// Defines a protocol and its request and notification types.
///
/// Entries with `-> Reply` generate a [`Request`]; other entries generate a
/// [`Notification`]. Each entry is a unit type or has one tuple field. Use
/// `-> ()` for a request whose reply has no data.
///
/// ```
/// use samara::prelude::*;
/// protocol! {
///     pub type Health => enum HealthMessage {
///         Changed(String),
///         Disconnected,
///         Read -> bool,
///     }
/// }
/// let mut program = Program::builder();
/// let health = program.port::<Health>(PortId::new("health"));
/// let command = Command::request_with(health, Read, |outcome| outcome);
/// assert_eq!(command.request_intents::<Read>().len(), 1);
/// ```
///
/// The generated message enum carries a notification's field directly, or a
/// [`RequestInvocation`] for a request. All generated types implement `Debug`.
/// Per-operation attributes, generics, and multiple fields are not supported;
/// implement [`Protocol`], [`Request`], and [`Notification`] yourself for those cases.
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

        #[doc = concat!("Messages accepted by providers of `", stringify!($protocol), "`.")]
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
                #[doc = concat!("Request arguments for `", stringify!($protocol), "`.")]
                #[derive(Debug)]
                $visibility struct $operation(
                    #[doc = "Request payload."]
                    $visibility $payload
                );
            ]
            [
                $($message_variants)*
                #[doc = concat!("Request `", stringify!($operation), "`.")]
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
                #[doc = concat!("Request without arguments for `", stringify!($protocol), "`.")]
                #[derive(Debug)]
                $visibility struct $operation;
            ]
            [
                $($message_variants)*
                #[doc = concat!("Request `", stringify!($operation), "`.")]
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
                #[doc = concat!("Notification arguments for `", stringify!($protocol), "`.")]
                #[derive(Debug)]
                $visibility struct $operation(
                    #[doc = "Notification payload."]
                    $visibility $payload
                );
            ]
            [
                $($message_variants)*
                #[doc = concat!("Notification `", stringify!($operation), "`.")]
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
                #[doc = concat!("Notification without arguments for `", stringify!($protocol), "`.")]
                #[derive(Debug)]
                $visibility struct $operation;
            ]
            [
                $($message_variants)*
                #[doc = concat!("Notification `", stringify!($operation), "`.")]
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
    fn capability(&self) -> &CapabilityToken;
    fn intent(&self) -> &dyn Any;
    fn intent_type_name(&self) -> &'static str;
    fn maps_directly_to_message(&self) -> bool;
    fn into_parts(self: Box<Self>) -> (Box<dyn Any + Send>, Option<ErasedEffectMapper<Message>>);
}

// Pure Layer continuations may describe another finite command; only Message
// continuations enter the Component transition path.
enum EffectContinuation<Message> {
    Message(Message),
    Command(Command<Message>),
}

type ErasedEffectMapper<Message> =
    Box<dyn FnOnce(Box<dyn Any + Send>) -> EffectContinuation<Message> + Send + 'static>;

type EffectMapper<E, Message> = Box<
    dyn FnOnce(
            EffectOutcome<<E as EffectDescriptor>::Output, <E as EffectDescriptor>::Error>,
        ) -> Message
        + Send
        + 'static,
>;

struct Perform<E, Map> {
    direct_message: bool,
    capability: CapabilityToken,
    effect: E,
    map: Map,
}

impl<Message, E, Map> ErasedEffectCommand<Message> for Perform<E, Map>
where
    Message: Send + 'static,
    E: EffectDescriptor,
    Map: FnOnce(EffectOutcome<E::Output, E::Error>) -> EffectContinuation<Message> + Send + 'static,
{
    fn capability(&self) -> &CapabilityToken {
        &self.capability
    }

    fn intent(&self) -> &dyn Any {
        &self.effect
    }

    fn intent_type_name(&self) -> &'static str {
        std::any::type_name::<E>()
    }

    fn maps_directly_to_message(&self) -> bool {
        self.direct_message
    }

    fn into_parts(self: Box<Self>) -> (Box<dyn Any + Send>, Option<ErasedEffectMapper<Message>>) {
        let Self { effect, map, .. } = *self;
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
    capability: CapabilityToken,
    effect: E,
}

impl<Message, E> ErasedEffectCommand<Message> for PerformDiscardingOutcome<E>
where
    E: EffectDescriptor,
{
    fn capability(&self) -> &CapabilityToken {
        &self.capability
    }

    fn intent(&self) -> &dyn Any {
        &self.effect
    }

    fn intent_type_name(&self) -> &'static str {
        std::any::type_name::<E>()
    }

    fn maps_directly_to_message(&self) -> bool {
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
    fn timeout(&self) -> Option<Duration>;
    fn request_type_name(&self) -> &'static str;
    fn reply_type_name(&self) -> &'static str;
    fn reply_type_id(&self) -> TypeId;
    fn protocol_type_id(&self) -> TypeId;
    fn protocol_type_name(&self) -> &'static str;
    fn mapper_type_name(&self) -> &'static str;
    fn into_parts(
        self: Box<Self>,
        token: Arc<RequestToken>,
    ) -> (Box<dyn Any + Send>, ErasedRequestMapper<Message>);
}

type ErasedRequestMapper<Message> =
    Box<dyn FnOnce(RequestOutcome<Box<dyn Any + Send>>) -> Message + Send + 'static>;

fn typed_request_outcome<Reply: Send + 'static>(
    outcome: RequestOutcome<Box<dyn Any + Send>>,
) -> RequestOutcome<Reply> {
    match outcome {
        RequestOutcome::Replied(reply) => RequestOutcome::Replied(
            *reply
                .downcast::<Reply>()
                .expect("Request couples correlation to its Reply type"),
        ),
        RequestOutcome::TimedOut => RequestOutcome::TimedOut,
        RequestOutcome::Failed(error) => RequestOutcome::Failed(error),
        RequestOutcome::Cancelled => RequestOutcome::Cancelled,
    }
}

struct RequestCommand<P, R, Map>
where
    P: Protocol,
    R: Request<P>,
{
    port: Port<P>,
    request: R,
    timeout: Option<Duration>,
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

    fn timeout(&self) -> Option<Duration> {
        self.timeout
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
        token: Arc<RequestToken>,
    ) -> (Box<dyn Any + Send>, ErasedRequestMapper<Message>) {
        let Self { request, map, .. } = *self;
        let message = request.into_message(ReplyTo::runtime(token));
        let mapper = Box::new(move |outcome| map(typed_request_outcome::<R::Reply>(outcome)));
        (Box::new(message), mapper)
    }
}

trait ErasedReplyCommand: Send {
    fn reply(&self) -> &dyn Any;
    fn reply_type_name(&self) -> &'static str;
    fn reply_type_id(&self) -> TypeId;
    fn into_parts(self: Box<Self>) -> (Arc<RequestToken>, Box<dyn Any + Send>);
}

struct Reply<Reply> {
    reply_to: ReplyTo<Reply>,
    reply: Reply,
}

/// An effect and its response mapper, extracted for a unit test.
///
/// Obtain it with [`Command::into_effect`], inspect the descriptor, then supply a
/// result with [`Self::map_outcome`]. No I/O runs and no message is delivered.
/// For effects that discard results, use [`Command::effect_intent`] instead.
///
/// ```
/// use samara::prelude::*;
/// let mut program = Program::builder();
/// let output = program.effect::<PrintStdout>();
/// let command = Command::effect_with(&output, PrintStdout::line("ready"), |outcome| outcome);
/// let invocation = command.into_effect::<PrintStdout>().ok().unwrap();
/// assert_eq!(invocation.descriptor().as_str(), "ready\n");
/// assert!(matches!(invocation.map_outcome(EffectOutcome::Succeeded(())),
///                  EffectOutcome::Succeeded(())));
/// ```
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
    /// Returns the effect arguments for inspection.
    pub fn descriptor(&self) -> &E {
        &self.descriptor
    }

    /// Converts a supplied effect result to a message, consuming the mapper.
    ///
    /// See [`EffectInvocation`] for an example. Calling this does not run the effect
    /// or deliver the message.
    ///
    /// <details>
    /// <summary>Compile-time checks</summary>
    ///
    /// ```compile_fail
    /// use samara::{Command, EffectDescriptor, EffectOutcome, Program};
    ///
    /// struct Read;
    /// impl EffectDescriptor for Read {
    ///     type Output = ();
    ///     type Error = ();
    /// }
    ///
    /// let mut program = Program::builder();
    /// let read = program.effect::<Read>();
    /// let invocation = Command::effect_with(&read, Read, |_| ())
    ///     .into_effect::<Read>()
    ///     .ok()
    ///     .unwrap();
    /// invocation.map_outcome(EffectOutcome::Succeeded(()));
    /// invocation.map_outcome(EffectOutcome::Succeeded(()));
    /// ```
    ///
    /// </details>
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

    fn into_parts(self: Box<Self>) -> (Arc<RequestToken>, Box<dyn Any + Send>) {
        (self.reply_to.token, Box::new(self.reply))
    }
}

/// Actions for Samara to run after a component update or at startup.
///
/// Return a Command from [`Component::update`] or attach it to [`Init`]. Merely
/// constructing it does not perform the action. Use [`Self::batch`] for several
/// actions and [`Self::none`] when no work is needed.
///
/// ```
/// use samara::prelude::*;
/// use std::time::Duration;
/// let mut program = Program::builder();
/// let output = program.effect::<PrintStdout>();
/// let command = Command::batch([
///     samara::println!(&output, "started"),
///     Command::after(Duration::from_secs(1), ()),
/// ]);
/// assert_eq!(command.effect_intents::<PrintStdout>().len(), 1);
/// ```
///
/// An effect's mapper converts its result to a message. Keep the mapper free of
/// I/O and state changes; handle the message in `update`. Commands cannot be
/// cloned or compared because they can own these mappers. Test their contents
/// with methods such as [`Self::effect_intent`] and [`Self::request_intents`].
///
/// <details>
/// <summary>Compile-time checks</summary>
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
///
/// </details>
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
    /// Returns a command that does nothing.
    pub fn none() -> Self {
        Self(CommandKind::None)
    }

    /// Requests an effect and converts its result with `Message::from`.
    ///
    /// The capability must come from the component's Program. Use
    /// [`Self::effect_with`] to choose the message conversion at the call site.
    ///
    /// ```
    /// use samara::prelude::*;
    /// enum Message { Printed(EffectOutcome<(), std::convert::Infallible>) }
    /// impl From<EffectOutcome<(), std::convert::Infallible>> for Message {
    ///     fn from(outcome: EffectOutcome<(), std::convert::Infallible>) -> Self {
    ///         Self::Printed(outcome)
    ///     }
    /// }
    /// let mut program = Program::builder();
    /// let output = program.effect::<PrintStdout>();
    /// let command: Command<Message> = Command::effect(&output, PrintStdout::line("ready"));
    /// ```
    pub fn effect<E>(capability: &EffectCapability<E>, effect: E) -> Self
    where
        Message: From<EffectOutcome<E::Output, E::Error>> + Send + 'static,
        E: EffectDescriptor,
    {
        Self::effect_with(capability, effect, Message::from)
    }

    /// Requests an effect and maps its result to a message.
    ///
    /// `map` runs at most once and may capture values such as a request ID. It must
    /// not perform I/O or change application state. Runtime cancellation drops the
    /// mapper without calling it. The capability must come from the same Program.
    ///
    /// ```
    /// use samara::prelude::*;
    /// let mut program = Program::builder();
    /// let http = program.effect::<HttpRequest>();
    /// let revision = 3;
    /// let command = Command::effect_with(
    ///     &http, HttpRequest::get("https://example.test/"),
    ///     move |outcome| (revision, outcome),
    /// );
    /// assert_eq!(command.effect_intent::<HttpRequest>().unwrap().url(), "https://example.test/");
    /// ```
    ///
    /// Controlled tests inspect the request with [`ControlledRuntime::next_effect`]
    /// and provide its result with [`ControlledRuntime::complete`].
    ///
    /// <details>
    /// <summary>Compile-time checks</summary>
    ///
    /// ```compile_fail
    /// use samara::{Command, EffectDescriptor, Program};
    ///
    /// struct Read;
    /// impl EffectDescriptor for Read {
    ///     type Output = ();
    ///     type Error = ();
    /// }
    /// struct HiddenWork;
    /// let mut program = Program::builder();
    /// let read = program.effect::<Read>();
    /// let _: Command<()> = Command::effect_with(&read, HiddenWork, |_| ());
    /// ```
    ///
    /// ```compile_fail
    /// use samara::{Command, EffectDescriptor, Program};
    ///
    /// struct Read;
    /// impl EffectDescriptor for Read {
    ///     type Output = ();
    ///     type Error = ();
    /// }
    ///
    /// let mut program = Program::builder();
    /// let read = program.effect::<Read>();
    /// let _: Command<()> = Command::effect_with(&read, Read, |_| async {});
    /// ```
    ///
    /// </details>
    pub fn effect_with<E, Map>(capability: &EffectCapability<E>, effect: E, map: Map) -> Self
    where
        Message: Send + 'static,
        E: EffectDescriptor,
        Map: FnOnce(EffectOutcome<E::Output, E::Error>) -> Message + Send + 'static,
    {
        Self(CommandKind::Effect(Box::new(Perform {
            direct_message: true,
            capability: capability.token.clone(),
            effect,
            map: move |outcome| EffectContinuation::Message(map(outcome)),
        })))
    }

    /// Requests an effect without sending its result to the component.
    ///
    /// Samara still tracks the work: Drain waits for it, Cancel aborts it, and
    /// controlled tests must complete or cancel it.
    ///
    /// ```
    /// use samara::prelude::*;
    /// let mut program = Program::builder();
    /// let output = program.effect::<PrintStdout>();
    /// let command: Command<()> = Command::effect_discarding_outcome(&output, PrintStdout::line("ready"));
    /// assert_eq!(command.effect_intent::<PrintStdout>().unwrap().as_str(), "ready\n");
    /// ```
    pub fn effect_discarding_outcome<E>(capability: &EffectCapability<E>, effect: E) -> Self
    where
        E: EffectDescriptor,
    {
        Self(CommandKind::Effect(Box::new(PerformDiscardingOutcome {
            capability: capability.token.clone(),
            effect,
        })))
    }

    /// Queues a message for another component.
    ///
    /// The recipient handles it after the current update returns. No delivery result
    /// is sent back; a routing error faults the runtime. Use [`Self::notify`] or
    /// [`Self::request`] to depend on a protocol instead of another component's type.
    ///
    /// ```
    /// use samara::{Command, Component, ComponentRef};
    /// fn forward<C: Component>(target: ComponentRef<C>, message: C::Message) -> Command<()> {
    ///     Command::send(target, message)
    /// }
    /// ```
    pub fn send<C>(target: ComponentRef<C>, message: C::Message) -> Self
    where
        C: Component,
    {
        Self(CommandKind::Send(Box::new(SendTo { target, message })))
    }

    /// Sends a notification to the component bound to a Port.
    ///
    /// The notification is converted with [`Notification::into_message`]. The command
    /// schedules delivery; it does not call the provider while being constructed.
    /// No reply or delivery result is sent back. See [`Notification`] for an example.
    pub fn notify<P, N>(port: Port<P>, notification: N) -> Self
    where
        P: Protocol,
        N: Notification<P>,
    {
        Self(CommandKind::Notify(Box::new(Notify { port, notification })))
    }

    /// Sends a request through a Port and converts the reply with `Message::from`.
    ///
    /// Requires `Message: From<RequestOutcome<R::Reply>>`. Use [`Self::request_with`]
    /// for a closure, or [`Self::request_timeout`] to limit how long to wait.
    pub fn request<P, R>(port: Port<P>, request: R) -> Self
    where
        Message: From<RequestOutcome<R::Reply>> + Send + 'static,
        P: Protocol,
        R: Request<P>,
    {
        Self::request_with(port, request, Message::from)
    }

    /// Sends a request through a Port and maps its reply to a message.
    ///
    /// The component continues handling other messages while waiting. `map` runs at
    /// most once and must not perform I/O or mutate state. You can capture an ID to
    /// associate the reply with application work.
    ///
    /// ```
    /// use samara::prelude::*;
    /// protocol! { type Catalog => enum CatalogMessage { Lookup(String) -> Option<u64>, } }
    /// let mut program = Program::builder();
    /// let catalog = program.port::<Catalog>(PortId::new("catalog"));
    /// let command = Command::request_with(catalog, Lookup("widget".into()), |reply| (7, reply));
    /// assert_eq!(command.request_intents::<Lookup>().len(), 1);
    /// ```
    ///
    /// There is no timeout. An unanswered request can keep [`Shutdown::Drain`] waiting;
    /// use [`Self::request_timeout_with`] to bound the wait.
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
            timeout: None,
            map,
        })))
    }

    /// Sends a request with a timeout and converts its result with `Message::from`.
    ///
    /// The timeout starts when Samara runs the command. A reply must be processed
    /// before the deadline; a zero timeout always expires. On expiry, the caller gets
    /// [`RequestOutcome::TimedOut`], the provider keeps running, and later replies are
    /// ignored. A duration that overflows the clock faults the runtime.
    ///
    /// Live execution uses Tokio time; controlled tests advance time themselves.
    /// See [`Self::request_timeout_with`] for a closure-based example.
    pub fn request_timeout<P, R>(port: Port<P>, request: R, timeout: Duration) -> Self
    where
        Message: From<RequestOutcome<R::Reply>> + Send + 'static,
        P: Protocol,
        R: Request<P>,
    {
        Self::request_timeout_with(port, request, timeout, Message::from)
    }

    /// Sends a request with a timeout and maps its reply or timeout to a message.
    ///
    /// Uses the deadline rules of [`Self::request_timeout`]. Runtime cancellation
    /// drops the mapper without sending a message.
    ///
    /// ```
    /// use samara::prelude::*;
    /// use std::time::Duration;
    /// protocol! { type Health => enum HealthMessage { Check -> bool, } }
    /// let mut program = Program::builder();
    /// let health = program.port::<Health>(PortId::new("health"));
    /// let command = Command::request_timeout_with(
    ///     health, Check, Duration::from_secs(2), |outcome| outcome,
    /// );
    /// assert_eq!(command.request_intents::<Check>().len(), 1);
    /// ```
    pub fn request_timeout_with<P, R, Map>(
        port: Port<P>,
        request: R,
        timeout: Duration,
        map: Map,
    ) -> Self
    where
        Message: Send + 'static,
        P: Protocol,
        R: Request<P>,
        Map: FnOnce(RequestOutcome<R::Reply>) -> Message + Send + 'static,
    {
        Self(CommandKind::Request(Box::new(RequestCommand {
            port,
            request,
            timeout: Some(timeout),
            map,
        })))
    }

    /// Replies to a request using its reply token.
    ///
    /// The token is consumed, so the same request cannot be answered twice. Delivery
    /// happens after the provider's update returns. A reply after the caller's timeout
    /// is ignored. See [`ProgramBuilder::bind_port`] for an example.
    pub fn reply<ReplyValue>(reply_to: ReplyTo<ReplyValue>, reply: ReplyValue) -> Self
    where
        ReplyValue: Send + 'static,
    {
        Self(CommandKind::Reply(Box::new(Reply { reply_to, reply })))
    }

    /// Schedules a message for this component after `delay`.
    ///
    /// Uses Tokio time live and the test-controlled clock in [`ControlledRuntime`].
    /// The delay starts when Samara runs the command, not when you construct it.
    ///
    /// ```
    /// use samara::Command;
    /// use std::time::Duration;
    /// enum Message { Tick }
    /// let command = Command::after(Duration::from_secs(1), Message::Tick);
    /// ```
    pub const fn after(delay: Duration, message: Message) -> Self {
        Self(CommandKind::After { delay, message })
    }

    /// Groups actions to run after the same update.
    ///
    /// Actions are independent: completion order is not guaranteed. A timer batched
    /// with an HTTP request starts alongside it, rather than waiting for the response.
    /// To sequence work, issue the next command when handling the previous result.
    ///
    /// See [`Command`] for a batch example.
    pub fn batch(commands: impl IntoIterator<Item = Self>) -> Self {
        Self(CommandKind::Batch(commands.into_iter().collect()))
    }

    /// Returns whether this is `Command::none()`. An empty batch returns `false`.
    pub fn is_none(&self) -> bool {
        matches!(&self.0, CommandKind::None)
    }

    /// Returns the first effect descriptor of type `E`, searching inside batches.
    ///
    /// Useful for testing an update without running I/O. See [`Self::effect_with`].
    pub fn effect_intent<E: EffectDescriptor>(&self) -> Option<&E> {
        self.effect_intents::<E>().into_iter().next()
    }

    /// Returns all effect descriptors of type `E`, searching inside batches.
    ///
    /// Results follow declaration order, including duplicates. This order does not
    /// predict execution or completion order.
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

    /// Maps a supplied effect result to a message for a unit test.
    ///
    /// Consumes a top-level effect command. Returns the command unchanged if its type
    /// does not match, it discards its result, or it may issue another command (such
    /// as a redirect-following pipeline). Batches must be unpacked first with
    /// [`Self::into_declarations`]. No I/O runs. See [`EffectInvocation`] for an example.
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

    /// Extracts an effect and its mapper for a unit test.
    ///
    /// Returns the command unchanged if it is a batch, has a different descriptor
    /// type, discards its result, or may issue another command. Use
    /// [`Self::into_declarations`] for batches and [`ControlledRuntime`] to test
    /// redirect-following pipelines. See [`EffectInvocation`] for an example.
    pub fn into_effect<E>(self) -> Result<EffectInvocation<E, Message>, Self>
    where
        Message: Send + 'static,
        E: EffectDescriptor,
    {
        match self.0 {
            CommandKind::Effect(command)
                if command.intent().is::<E>() && command.maps_directly_to_message() =>
            {
                let (descriptor, mapper) = command.into_parts();
                let descriptor = match descriptor.downcast::<E>() {
                    Ok(descriptor) => *descriptor,
                    Err(_) => unreachable!("the descriptor type was checked before interception"),
                };
                let mapper = mapper.expect("mapped effects retain one outcome mapper");
                let mapper = Box::new(move |outcome| match mapper(Box::new(outcome)) {
                    EffectContinuation::Message(message) => message,
                    EffectContinuation::Command(_) => {
                        unreachable!("direct Message mapper checked above")
                    }
                });
                Ok(EffectInvocation { descriptor, mapper })
            }
            command => Err(Self(command)),
        }
    }

    /// Returns notifications of type `N` with their destination Port IDs.
    ///
    /// Searches batches in declaration order without delivering anything. See
    /// [`Notification`] for an example.
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

    /// Returns requests of type `R` with their destination Port IDs.
    ///
    /// Searches batches in declaration order. See [`Self::request_with`] for an example.
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

    /// Returns replies of type `ReplyValue`, searching inside batches in declaration order.
    ///
    /// This inspects reply values without consuming tokens or delivering replies.
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

    /// Returns the Rust type name of a top-level effect descriptor.
    ///
    /// Returns `None` for other commands, including batches. The name is diagnostic
    /// text, not a stable identifier for storage or communication.
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

    /// Unpacks batches and removes `none` commands.
    ///
    /// The returned list follows declaration order. It can be used to inspect each
    /// action separately; it does not promise an execution order.
    ///
    /// ```
    /// use samara::Command;
    /// use std::time::Duration;
    /// let command = Command::batch([Command::none(), Command::after(Duration::ZERO, ())]);
    /// assert_eq!(command.into_declarations().len(), 1);
    /// ```
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
    fn capability(&self) -> &CapabilityToken;
    fn descriptor(&self) -> &dyn Any;
    fn descriptor_snapshot(&self) -> Box<dyn ErasedSourceDescriptor>;
    fn source_plan(&self) -> SourcePlan;
    fn source_event_type_id(&self) -> TypeId;
    fn descriptor_type_name(&self) -> &'static str;
    fn mapper_type_name(&self) -> &'static str;
    fn map_event(&self, event: Box<dyn Any + Send>) -> Message;
}

struct MappedSourceDescriptor<S, Map> {
    capability: CapabilityToken,
    descriptor: S,
    map: Map,
}

impl<Message, S, Map> ErasedSubscription<Message> for MappedSourceDescriptor<S, Map>
where
    Message: Send + 'static,
    S: SourceDescriptor,
    Map: Fn(SourceEvent<S::Item, S::Error>) -> Message + Send + Sync + 'static,
{
    fn capability(&self) -> &CapabilityToken {
        &self.capability
    }

    fn descriptor(&self) -> &dyn Any {
        &self.descriptor
    }

    fn descriptor_snapshot(&self) -> Box<dyn ErasedSourceDescriptor> {
        Box::new(SourceDescriptorSnapshot {
            capability: self.capability.clone(),
            descriptor: self.descriptor.clone(),
        })
    }

    fn source_plan(&self) -> SourcePlan {
        let mut plan = self
            .descriptor
            .__samara_source_plan(source_plan_private::LOWER_TOKEN);
        plan.capability = Some(self.capability.clone());
        plan
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
    fn matches(&self, capability: &CapabilityToken, descriptor: &dyn Any) -> bool;
}

struct SourceDescriptorSnapshot<S: SourceDescriptor> {
    capability: CapabilityToken,
    descriptor: S,
}

impl<S: SourceDescriptor> ErasedSourceDescriptor for SourceDescriptorSnapshot<S> {
    fn matches(&self, capability: &CapabilityToken, descriptor: &dyn Any) -> bool {
        self.capability.same_as(capability)
            && descriptor.downcast_ref::<S>() == Some(&self.descriptor)
    }
}

/// An input a component wants to receive, with a name and an event-to-message mapper.
///
/// Return subscriptions from [`Component::subscriptions`]. Samara compares them
/// after each update:
///
/// - The same ID, capability, and equal descriptor keep the input running.
/// - A changed capability or descriptor stops the old input and starts a new one.
/// - An omitted ID stops that input.
///
/// IDs must be unique within one component's returned set. Updating only the
/// mapper keeps the input running and uses the new mapper for subsequent events;
/// messages already created keep their original values. Replacing the input
/// discards old events and messages that have not begun an update.
///
/// ```
/// use samara::prelude::*;
/// enum Message { Input(SourceEvent<String, StdinError>) }
/// let mut program = Program::builder();
/// let input = program.source::<StdinLines>();
/// let subscription = Subscription::source_with(
///     &input, SubscriptionId::new("commands"), StdinLines::new(), Message::Input,
/// );
/// assert_eq!(subscription.source_descriptor::<StdinLines>(), Some(&StdinLines::new()));
/// ```
///
/// See [`StdinLines`] for a complete component, live binding, and controlled test.
pub struct Subscription<Message> {
    id: SubscriptionId,
    descriptor: Box<dyn ErasedSubscription<Message>>,
}

impl<Message> Subscription<Message> {
    /// Subscribes to a source and converts each event with `Message::from`.
    ///
    /// Requires `Message: From<SourceEvent<S::Item, S::Error>>`. Use
    /// [`Self::source_with`] to supply a closure. See [`StdinLines`] for a complete example.
    pub fn source<S>(capability: &SourceCapability<S>, id: SubscriptionId, descriptor: S) -> Self
    where
        Message: From<SourceEvent<S::Item, S::Error>> + Send + 'static,
        S: SourceDescriptor,
    {
        Self::source_with(capability, id, descriptor, Message::from)
    }

    /// Subscribes to a source and maps each event to a message.
    ///
    /// The capability must come from the component's Program. The mapper may be called
    /// many times and must not perform I/O or change state. Capture configuration or
    /// IDs by value when events need application context. See [`Subscription`] for a
    /// construction example and [`StdinLines`] for live and controlled wiring.
    pub fn source_with<S, Map>(
        capability: &SourceCapability<S>,
        id: SubscriptionId,
        descriptor: S,
        map: Map,
    ) -> Self
    where
        Message: Send + 'static,
        S: SourceDescriptor,
        Map: Fn(SourceEvent<S::Item, S::Error>) -> Message + Send + Sync + 'static,
    {
        Self {
            id,
            descriptor: Box::new(MappedSourceDescriptor {
                capability: capability.token.clone(),
                descriptor,
                map,
            }),
        }
    }

    /// Returns the name used to match this subscription across updates.
    pub fn id(&self) -> &SubscriptionId {
        &self.id
    }

    /// Returns this subscription's descriptor if it has type `S`, otherwise `None`.
    ///
    /// Use it to inspect subscriptions without starting the input.
    pub fn source_descriptor<S: SourceDescriptor>(&self) -> Option<&S> {
        self.descriptor.descriptor().downcast_ref()
    }

    /// Maps a test event to a message without starting a source or delivering the message.
    ///
    /// The mapper can be called repeatedly. If the descriptor is not type `S`, returns
    /// the event unchanged.
    ///
    /// ```
    /// use samara::prelude::*;
    /// let mut program = Program::builder();
    /// let input = program.source::<StdinLines>();
    /// let subscription = Subscription::source_with(
    ///     &input, SubscriptionId::new("input"), StdinLines::new(), |event| event,
    /// );
    /// let message = subscription.map_source_event::<StdinLines>(SourceEvent::Item("hello".into())).unwrap();
    /// assert_eq!(message, SourceEvent::Item("hello".into()));
    /// ```
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

    /// Returns the descriptor's Rust type name for diagnostics.
    ///
    /// The name is not a stable identifier for storage or communication.
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
        descriptor.matches(self.descriptor.capability(), self.descriptor.descriptor())
    }
}

/// The inputs a component wants to keep active.
///
/// Return the full set on each call to [`Component::subscriptions`]. Leaving out
/// an earlier subscription stops its input. IDs must be unique within the set.
///
/// ```
/// use samara::prelude::*;
/// let mut program = Program::builder();
/// let input = program.source::<StdinLines>();
/// let subscriptions: Subscriptions<SourceEvent<String, StdinError>> = vec![
///     Subscription::source(&input, SubscriptionId::new("commands"), StdinLines::new()),
/// ].into();
/// assert_eq!(subscriptions.iter().count(), 1);
/// assert!(subscriptions.find::<StdinLines>(&SubscriptionId::new("commands")).is_some());
/// ```
pub struct Subscriptions<Message>(Vec<Subscription<Message>>);

impl<Message> Subscriptions<Message> {
    /// Returns an empty set of subscriptions.
    pub fn none() -> Self {
        Self(Vec::new())
    }

    /// Creates a set containing one subscription.
    pub fn one(subscription: Subscription<Message>) -> Self {
        Self(vec![subscription])
    }

    /// Iterates over the subscriptions in declaration order.
    pub fn iter(&self) -> impl Iterator<Item = &Subscription<Message>> {
        self.0.iter()
    }

    /// Returns the descriptor with this ID if it has type `S`, otherwise `None`.
    ///
    /// Useful for asserting what a component subscribes to. See [`Subscriptions`].
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

/// Describes values received from a Tokio `mpsc` channel.
///
/// The capability selects the channel. The name is configuration used to compare
/// subscriptions, not a global channel lookup key.
///
/// ```
/// use samara::prelude::*;
/// use std::convert::Infallible;
/// struct Sum { input: SourceCapability<StreamDescriptor<u64>> }
/// impl Component for Sum {
///     type Model = u64;
///     type Message = SourceEvent<u64, Infallible>;
///     fn init(&self) -> Init<u64, Self::Message> { Init::new(0) }
///     fn update(&self, sum: &mut u64, event: Self::Message) -> Command<Self::Message> {
///         if let SourceEvent::Item(value) = event { *sum += value; }
///         Command::none()
///     }
///     fn subscriptions(&self, _: &u64) -> Subscriptions<Self::Message> {
///         Subscriptions::one(Subscription::source(
///             &self.input, SubscriptionId::new("values"), StreamDescriptor::named("numbers"),
///         ))
///     }
/// }
/// fn program() -> Result<(Program, ComponentRef<Sum>, SourceCapability<StreamDescriptor<u64>>), ProgramBuildError> {
///     let mut builder = Program::builder();
///     let input = builder.source::<StreamDescriptor<u64>>();
///     let sum = builder.component(ComponentId::new("sum"), Sum { input: input.clone() });
///     Ok((builder.build()?, sum, input))
/// }
/// let (program_under_test, sum, input) = program()?;
/// let mut test = ControlledRuntime::builder(program_under_test).control_stream(&input).build()?;
/// test.run_until_idle()?;
/// test.emit_stream(&input, 3)?;
/// test.close_stream(&input)?;
/// test.run_until_idle()?;
/// assert_eq!(*test.state(&sum)?, 3);
/// test.cancel()?;
///
/// // In the host, bind a real receiver instead.
/// let (program, _, input) = program()?;
/// let (sender, receiver) = tokio::sync::mpsc::channel(16);
/// let live = LiveRuntime::builder(program).bind_mpsc(&input, receiver).build()?;
/// // Inside Tokio, call live.spawn() and sender.send(value).await.
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// A bound receiver can be used by only one subscription and cannot be restarted
/// after ending or being stopped. A second activation faults the live runtime.
/// Channel closure produces [`SourceEvent::Ended`].
pub struct StreamDescriptor<T> {
    binding: Arc<str>,
    marker: PhantomData<fn() -> T>,
}

impl<T> StreamDescriptor<T> {
    /// Creates a descriptor with this name. It does not create or bind a channel.
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

    #[allow(private_interfaces)]
    fn __samara_terminal_stream_item_type_id(_: source_plan_private::LowerToken) -> Option<TypeId> {
        Some(TypeId::of::<T>())
    }
}

impl<T: Send + 'static> stream_source_private::Sealed for StreamDescriptor<T> {}

impl<T: Send + 'static> StreamSourceDescriptor for StreamDescriptor<T> {
    type StreamItem = T;
}

/// Reads UTF-8 lines from process standard input.
///
/// Each item has its trailing LF and preceding CR removed. Empty lines are kept.
/// EOF emits any remaining nonempty line, then [`SourceEvent::Ended`]. A read or
/// UTF-8 error produces [`SourceEvent::Failed`] and stops reading. Lines have no
/// size limit. Live input is available on Unix through
/// `LiveRuntimeBuilder::bind_stdin`; tests can supply events on any platform.
///
/// The following example declares the input, stores its capability on a component,
/// returns a subscription, and runs that component under test control. It also
/// shows the live binding for the same program.
///
/// ```
/// use samara::prelude::*;
///
/// struct Reader { input: SourceCapability<StdinLines> }
/// struct Model { lines: Vec<String>, error: Option<StdinError>, open: bool }
///
/// impl Component for Reader {
///     type Model = Model;
///     type Message = SourceEvent<String, StdinError>;
///
///     fn init(&self) -> Init<Model, Self::Message> {
///         Init::new(Model { lines: Vec::new(), error: None, open: true })
///     }
///     fn update(&self, model: &mut Model, event: Self::Message) -> Command<Self::Message> {
///         match event {
///             SourceEvent::Item(line) => model.lines.push(line),
///             SourceEvent::Ended => model.open = false,
///             SourceEvent::Failed(error) => {
///                 model.error = Some(error);
///                 model.open = false;
///             }
///         }
///         Command::none()
///     }
///     fn subscriptions(&self, model: &Model) -> Subscriptions<Self::Message> {
///         if !model.open { return Subscriptions::none(); }
///         Subscriptions::one(Subscription::source(
///             &self.input, SubscriptionId::new("input"), StdinLines::new(),
///         ))
///     }
/// }
///
/// fn program() -> Result<(Program, ComponentRef<Reader>, SourceCapability<StdinLines>), ProgramBuildError> {
///     let mut builder = Program::builder();
///     let input = builder.source::<StdinLines>();
///     let reader = builder.component(ComponentId::new("reader"), Reader { input: input.clone() });
///     Ok((builder.build()?, reader, input))
/// }
///
/// // Test input without reading the terminal.
/// let (program_under_test, reader, _) = program()?;
/// let mut test = ControlledRuntime::builder(program_under_test)
///     .control_source::<StdinLines>().build()?;
/// test.run_until_idle()?; // Starts the subscription.
/// test.emit_source::<_, StdinLines>(
///     &reader, &SubscriptionId::new("input"), SourceEvent::Item("hello".into()),
/// )?;
/// test.run_until_idle()?;
/// assert_eq!(test.state(&reader)?.lines, ["hello"]);
/// test.emit_source::<_, StdinLines>(&reader, &SubscriptionId::new("input"), SourceEvent::Ended)?;
/// test.run_until_idle()?;
/// assert!(!test.state(&reader)?.open);
/// assert!(test.cancel()?.is_clean());
///
/// // Bind the same component to process stdin on Unix.
/// #[cfg(unix)]
/// {
///     let (program, _, input) = program()?;
///     let live = LiveRuntime::builder(program).bind_stdin(&input).build()?;
///     // Call live.spawn() inside Tokio to start reading; see RuntimeTask for shutdown.
/// }
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// A source ending does not shut down the application. The host owns shutdown;
/// see [`RuntimeTask`]. Only one active subscription may read process stdin.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StdinLines;

impl StdinLines {
    /// Creates the descriptor. Reading starts only when the live subscription starts.
    pub const fn new() -> Self {
        Self
    }
}

impl SourceDescriptor for StdinLines {
    type Item = String;
    type Error = StdinError;
}

impl stdin_source_private::Sealed for StdinLines {}

impl StdinSourceDescriptor for StdinLines {}

/// The reason reading [`StdinLines`] stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StdinErrorKind {
    /// Reading bytes from process standard input failed.
    Read,
    /// One complete input line was not valid UTF-8.
    InvalidUtf8,
}

/// An input error with a category and diagnostic message.
///
/// ```
/// use samara::{StdinError, StdinErrorKind};
/// let error = StdinError::new(StdinErrorKind::InvalidUtf8, "input is not UTF-8");
/// assert_eq!(error.kind(), StdinErrorKind::InvalidUtf8);
/// assert_eq!(error.message(), "input is not UTF-8");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StdinError {
    kind: StdinErrorKind,
    message: Arc<str>,
}

impl StdinError {
    /// Creates an input error, for example to supply a failure in a controlled test.
    pub fn new(kind: StdinErrorKind, message: impl Into<Arc<str>>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    #[cfg(unix)]
    pub(crate) fn read(error: impl fmt::Display) -> Self {
        Self::new(StdinErrorKind::Read, error.to_string())
    }

    #[cfg(unix)]
    pub(crate) fn invalid_utf8(error: impl fmt::Display) -> Self {
        Self::new(StdinErrorKind::InvalidUtf8, error.to_string())
    }

    /// Returns the error category.
    pub fn kind(&self) -> StdinErrorKind {
        self.kind
    }

    /// Returns the diagnostic message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for StdinError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "standard-input {:?} error: {}",
            self.kind, self.message
        )
    }
}

impl Error for StdinError {}

/// Describes an HTTP request: method, URL, headers, and body.
///
/// Creating the request does not parse the URL or perform I/O. Use
/// [`Self::on_response`] to choose how to handle the response, then return the
/// resulting Command from your component. Live programs need
/// [`LiveRuntimeBuilder::bind_http`]; controlled tests use
/// `control_effect::<HttpRequest>()`.
///
/// ```
/// use samara::prelude::*;
/// let mut program = Program::builder();
/// let http = program.effect::<HttpRequest>();
/// let command = HttpRequest::get("https://example.test/health")
///     .on_response()
///     .require_success()
///     .into_command_with(&http, |outcome| outcome);
/// assert_eq!(command.effect_intent::<HttpRequest>().unwrap().url(),
///            "https://example.test/health");
/// ```
///
/// HTTP error statuses are responses, not transport failures. Use `require_success()`
/// to reject non-2xx statuses. Redirects are returned unchanged unless you choose
/// [`HttpResponsePipeline::follow_redirects`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpRequest {
    method: http::Method,
    url: Arc<str>,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl HttpRequest {
    /// Creates a request with the given method and URL, empty headers, and an empty body.
    ///
    /// Invalid URLs become [`HttpError`] values when the live driver runs the request.
    ///
    /// ```
    /// use samara::HttpRequest;
    /// let request = HttpRequest::new(http::Method::POST, "https://example.test/items")
    ///     .with_body("new item");
    /// assert_eq!(request.method(), http::Method::POST);
    /// assert_eq!(request.body().as_ref(), b"new item");
    /// ```
    pub fn new(method: http::Method, url: impl Into<Arc<str>>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: http::HeaderMap::new(),
            body: bytes::Bytes::new(),
        }
    }

    /// Creates a GET request with empty headers and an empty body. See [`HttpRequest`].
    pub fn get(url: impl Into<Arc<str>>) -> Self {
        Self::new(http::Method::GET, url)
    }

    /// Returns the request method.
    pub fn method(&self) -> &http::Method {
        &self.method
    }

    /// Returns the URL text supplied when the request was created.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Returns the request headers.
    pub fn headers(&self) -> &http::HeaderMap {
        &self.headers
    }

    /// Returns the request headers for editing.
    ///
    /// ```
    /// use samara::HttpRequest;
    /// let mut request = HttpRequest::get("https://example.test/");
    /// request.headers_mut().insert("accept", "application/json".parse().unwrap());
    /// assert_eq!(request.headers()["accept"], "application/json");
    /// ```
    pub fn headers_mut(&mut self) -> &mut http::HeaderMap {
        &mut self.headers
    }

    /// Appends a header, preserving existing values with the same name.
    ///
    /// Use [`Self::headers_mut`] and `insert` to replace a value.
    ///
    /// ```
    /// use samara::HttpRequest;
    /// let request = HttpRequest::get("https://example.test/")
    ///     .with_header(http::header::ACCEPT, "application/json".parse().unwrap());
    /// assert_eq!(request.headers()["accept"], "application/json");
    /// ```
    pub fn with_header(mut self, name: http::HeaderName, value: http::HeaderValue) -> Self {
        self.headers.append(name, value);
        self
    }

    /// Returns the request body.
    pub fn body(&self) -> &bytes::Bytes {
        &self.body
    }

    /// Replaces the request body.
    ///
    /// Does not set Content-Type; add that header when needed. See [`Self::new`].
    pub fn with_body(mut self, body: impl Into<bytes::Bytes>) -> Self {
        self.body = body.into();
        self
    }

    /// Starts a response pipeline for this request.
    ///
    /// Set the method, headers, and body before calling this. Then choose redirect,
    /// status, or JSON handling and convert the pipeline to a Command.
    ///
    /// ```
    /// use samara::prelude::*;
    /// let mut program = Program::builder();
    /// let http = program.effect::<HttpRequest>();
    /// let command = HttpRequest::get("https://example.test/config")
    ///     .on_response().require_success().json::<serde_json::Value>()
    ///     .into_command_with(&http, |outcome| outcome);
    /// ```
    ///
    /// <details>
    /// <summary>Compile-time checks</summary>
    ///
    /// ```compile_fail
    /// use samara::HttpRequest;
    ///
    /// HttpRequest::get("https://example.test")
    ///     .on_response()
    ///     .with_body("too late");
    /// ```
    ///
    /// </details>
    pub fn on_response(self) -> HttpResponsePipeline<HttpResponse, HttpError> {
        HttpResponsePipeline {
            request: self,
            redirect_limit: None,
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

/// An HTTP status, headers, version, and buffered response body.
///
/// All status codes can appear in a successful HTTP effect result, including
/// redirects and 4xx/5xx errors. Status checks and body decoding are available on
/// [`HttpResponsePipeline`].
///
/// Construct responses directly in controlled tests:
///
/// ```
/// use samara::{HttpResponse, StatusCode};
/// let response = HttpResponse::new(
///     StatusCode::OK, http::Version::HTTP_11, http::HeaderMap::new(), "ready",
/// );
/// assert!(response.status().is_success());
/// assert_eq!(response.version(), http::Version::HTTP_11);
/// assert!(response.headers().is_empty());
/// assert_eq!(response.into_body().as_ref(), b"ready");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    status: http::StatusCode,
    version: http::Version,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl HttpResponse {
    /// Creates a response from its parts, for example in a controlled test. See [`HttpResponse`].
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

    /// Returns the status code.
    pub fn status(&self) -> http::StatusCode {
        self.status
    }

    /// Returns the HTTP version.
    pub fn version(&self) -> http::Version {
        self.version
    }

    /// Returns the response headers.
    pub fn headers(&self) -> &http::HeaderMap {
        &self.headers
    }

    /// Returns the buffered response body.
    pub fn body(&self) -> &bytes::Bytes {
        &self.body
    }

    /// Consumes the response and returns its body.
    pub fn into_body(self) -> bytes::Bytes {
        self.body
    }
}

/// The reason an HTTP request or redirect attempt failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpErrorKind {
    /// The client or request could not be configured from the descriptor.
    Configuration,
    /// Sending the request or receiving its complete response failed.
    Transport,
    /// A redirect was rejected or exceeded the configured hop limit.
    Redirect,
}

/// An HTTP error with a category and diagnostic message.
///
/// HTTP status errors such as 404 are responses, not `HttpError` values.
/// [`HttpResponsePipeline::require_success`] can turn them into [`HttpStatusError`].
///
/// ```
/// use samara::{HttpError, HttpErrorKind};
/// let error = HttpError::new(HttpErrorKind::Transport, "connection reset");
/// assert_eq!(error.kind(), HttpErrorKind::Transport);
/// assert_eq!(error.message(), "connection reset");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpError {
    kind: HttpErrorKind,
    message: Arc<str>,
}

impl HttpError {
    /// Creates an HTTP error, for example to simulate a transport failure in a test.
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

    /// Returns the error category.
    pub fn kind(&self) -> HttpErrorKind {
        self.kind
    }

    /// Returns the diagnostic message.
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

/// Builds the response handling for an [`HttpRequest`].
///
/// Start with [`HttpRequest::on_response`], choose the handling steps, then call
/// [`Self::into_command`] or [`Self::into_command_with`]. Construction performs
/// no I/O. The pipeline owns the request and its response mapper and cannot be cloned.
///
/// ```
/// use samara::prelude::*;
/// let mut program = Program::builder();
/// let http = program.effect::<HttpRequest>();
/// let command = HttpRequest::get("https://example.test/config")
///     .on_response()
///     .follow_redirects(5)
///     .require_success()
///     .json::<serde_json::Value>()
///     .into_command_with(&http, |outcome| outcome);
/// ```
///
/// Steps run in the order shown. A failure skips later success transformations.
/// Redirect following issues each request through the same HTTP capability;
/// status checks and JSON decoding only run on the response returned to the caller.
///
/// <details>
/// <summary>Compile-time checks</summary>
///
/// ```compile_fail
/// use samara::HttpRequest;
///
/// let pipeline = HttpRequest::get("https://example.test").on_response();
/// let duplicate = pipeline.clone();
/// # drop(duplicate);
/// ```
///
/// </details>
#[must_use = "an HTTP response pipeline is inert until converted into a Command"]
pub struct HttpResponsePipeline<Output, ResponseError> {
    request: HttpRequest,
    redirect_limit: Option<usize>,
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
        let Self {
            request,
            redirect_limit,
            transform,
        } = self;
        HttpResponsePipeline {
            request,
            redirect_limit,
            transform: Box::new(move |outcome| transform_next(transform(outcome))),
        }
    }

    /// Creates a Command that converts the pipeline result with `Message::from`.
    ///
    /// Use [`Self::into_command_with`] for a closure. The capability must come from
    /// the component's Program.
    ///
    /// ```
    /// use samara::prelude::*;
    /// let mut program = Program::builder();
    /// let http = program.effect::<HttpRequest>();
    /// let command: Command<EffectOutcome<HttpResponse, HttpError>> =
    ///     HttpRequest::get("https://example.test/").on_response().into_command(&http);
    /// ```
    pub fn into_command<Message>(
        self,
        capability: &EffectCapability<HttpRequest>,
    ) -> Command<Message>
    where
        Message: From<EffectOutcome<Output, ResponseError>> + Send + 'static,
    {
        self.into_command_with(capability, Message::from)
    }

    /// Creates a Command that maps the pipeline result to a message.
    ///
    /// The mapper runs at most once and must not perform I/O or mutate state.
    /// If redirects are enabled, intermediate responses do not call it. Controlled
    /// tests inspect each HTTP request using `next_effect::<HttpRequest>()`.
    ///
    /// ```
    /// use samara::prelude::*;
    /// let mut program = Program::builder();
    /// let http = program.effect::<HttpRequest>();
    /// let revision = 7;
    /// let command = HttpRequest::get("https://example.test/").on_response()
    ///     .into_command_with(&http, move |outcome| (revision, outcome));
    /// ```
    pub fn into_command_with<Message, Map>(
        self,
        capability: &EffectCapability<HttpRequest>,
        map: Map,
    ) -> Command<Message>
    where
        Message: Send + 'static,
        Map: FnOnce(EffectOutcome<Output, ResponseError>) -> Message + Send + 'static,
    {
        let Self {
            request,
            redirect_limit,
            transform,
        } = self;
        match redirect_limit {
            Some(limit) => http_redirect::command(
                capability.clone(),
                request,
                limit,
                Box::new(move |outcome| map(transform(outcome))),
            ),
            None => {
                Command::effect_with(capability, request, move |outcome| map(transform(outcome)))
            }
        }
    }
}

impl HttpResponsePipeline<HttpResponse, HttpError> {
    /// Follows redirects, allowing at most `max_hops` additional requests.
    ///
    /// Call this before `require_success()` or `json()`. Another call replaces the
    /// limit. Zero allows the initial request but rejects any redirect it could follow.
    ///
    /// ```
    /// use samara::prelude::*;
    /// let mut program = Program::builder();
    /// let http = program.effect::<HttpRequest>();
    /// let command = HttpRequest::get("http://example.test/")
    ///     .on_response().follow_redirects(5).require_success()
    ///     .into_command_with(&http, |outcome| outcome);
    /// ```
    ///
    /// Follows 301, 302, 303, 307, and 308 responses with a Location header, resolving
    /// relative URLs against the current request. Other responses, including redirects
    /// without Location, are returned to the caller. POST becomes GET on 301/302;
    /// 303 uses GET except for HEAD and removes the body. Other methods on 301/302
    /// and all methods on 307/308 keep the body.
    ///
    /// An exhausted limit, malformed or multiple Locations, URL credentials, a
    /// non-HTTP(S) target, or an HTTPS-to-HTTP redirect produces [`HttpErrorKind::Redirect`].
    /// Host, Referer, and Proxy-Authorization are removed on every hop. A change of
    /// scheme, host, or effective port also removes Authorization, Cookie, Cookie2,
    /// and headers marked sensitive. Mark custom credential headers with
    /// `HeaderValue::set_sensitive(true)`.
    ///
    /// Each hop is a separate [`HttpRequest`] in controlled tests. Drain waits for
    /// the chain; Cancel stops it without a result message. The hop limit does not
    /// limit the time or size of a response.
    pub fn follow_redirects(mut self, max_hops: usize) -> Self {
        self.redirect_limit = Some(max_hops);
        self
    }

    /// Rejects responses outside the 200–299 status range.
    ///
    /// Returns [`HttpResponseError::Status`] with the response on rejection; later
    /// JSON decoding is skipped. Call this before [`Self::json`]. See
    /// [`HttpResponsePipeline`] for an example.
    ///
    /// <details>
    /// <summary>Compile-time checks</summary>
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
    ///
    /// </details>
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
    /// Decodes the response body as JSON into `T`.
    ///
    /// Accepts any HTTP status unless `require_success()` was called first. A decode
    /// error preserves the response and the `serde_json` error in [`HttpJsonError`].
    /// It does not check Content-Type.
    ///
    /// ```
    /// use samara::prelude::*;
    /// #[derive(serde::Deserialize)]
    /// struct Reply { count: u64 }
    /// let mut program = Program::builder();
    /// let http = program.effect::<HttpRequest>();
    /// let command = HttpRequest::get("https://example.test/count")
    ///     .on_response().require_success().json::<Reply>()
    ///     .into_command_with(&http, |outcome| outcome);
    /// ```
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

/// An HTTP request, status check, or JSON decoding error.
///
/// Use [`Self::http_error`] to inspect request failures and [`Self::response`] to
/// inspect responses rejected by status checks or JSON decoding. Cancellation
/// is represented separately by [`EffectOutcome::Cancelled`].
///
/// ```
/// use samara::{HttpError, HttpErrorKind, HttpResponseError};
/// let error: HttpResponseError = HttpError::new(HttpErrorKind::Redirect, "hop limit exceeded").into();
/// assert_eq!(error.http_error().unwrap().kind(), HttpErrorKind::Redirect);
/// assert!(error.response().is_none());
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HttpResponseError {
    /// The request could not be configured, transported, or redirected.
    #[error(transparent)]
    Http(#[from] HttpError),
    /// The response status was rejected by `require_success()`.
    #[error(transparent)]
    Status(#[from] HttpStatusError),
    /// The complete response body could not be decoded as JSON.
    #[error(transparent)]
    Json(#[from] HttpJsonError),
}

impl HttpResponseError {
    /// Returns the request or redirect error, or `None` for status and JSON errors.
    pub fn http_error(&self) -> Option<&HttpError> {
        match self {
            Self::Http(error) => Some(error),
            Self::Status(_) | Self::Json(_) => None,
        }
    }

    /// Returns the response rejected by a status check or JSON decoder.
    ///
    /// Returns `None` for request and redirect errors.
    pub fn response(&self) -> Option<&HttpResponse> {
        match self {
            Self::Http(_) => None,
            Self::Status(error) => Some(error.response()),
            Self::Json(error) => Some(error.response()),
        }
    }
}

/// A response rejected by [`HttpResponsePipeline::require_success`].
///
/// Use [`Self::status`] to branch on the status code and [`Self::response`] to
/// inspect headers or the body. The response is preserved unchanged.
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

    /// Returns the response associated with this error.
    pub fn response(&self) -> &HttpResponse {
        &self.response
    }

    /// Consumes the error and returns the response.
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

/// A JSON decoding error and the response that could not be decoded.
///
/// [`Self::source`] returns the `serde_json` error. [`Self::response`] gives access
/// to the body and headers for diagnostics; [`Self::into_parts`] returns both.
/// See [`HttpResponsePipeline::json`] for request construction.
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

    /// Returns the JSON decoder error.
    pub fn source(&self) -> &serde_json::Error {
        &self.source
    }

    /// Returns the response associated with this error.
    pub fn response(&self) -> &HttpResponse {
        &self.response
    }

    /// Consumes the error and returns the response.
    pub fn into_response(self) -> HttpResponse {
        self.response
    }

    /// Consumes the error and returns the JSON decoder error and response.
    pub fn into_parts(self) -> (serde_json::Error, HttpResponse) {
        (self.source, self.response)
    }
}

/// Text to write to standard output.
///
/// Creating this value does not write anything. Submit it with a Command and
/// bind output with [`LiveRuntimeBuilder::bind_stdio`]. The driver writes and
/// flushes the text but ignores I/O errors. Use [`println!`] for formatting.
///
/// ```
/// use samara::PrintStdout;
/// assert_eq!(PrintStdout::text("ready").as_str(), "ready");
/// assert_eq!(PrintStdout::line("ready").as_str(), "ready\n");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrintStdout {
    text: String,
}

impl PrintStdout {
    /// Creates a write with exactly this text and no added newline.
    pub fn text(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    /// Creates a write with one newline appended, even if the text already ends in one.
    pub fn line(text: impl Into<String>) -> Self {
        let mut text = text.into();
        text.push('\n');
        Self::text(text)
    }

    /// Returns the text that will be written.
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

/// Text to write to standard error.
///
/// Creating this value does not write anything. Submit it with a Command and
/// bind output with [`LiveRuntimeBuilder::bind_stdio`]. The driver writes and
/// flushes the text but ignores I/O errors. Use [`eprintln!`] for formatting.
///
/// ```
/// use samara::PrintStderr;
/// assert_eq!(PrintStderr::text("ready").as_str(), "ready");
/// assert_eq!(PrintStderr::line("ready").as_str(), "ready\n");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrintStderr {
    text: String,
}

impl PrintStderr {
    /// Creates a write with exactly this text and no added newline.
    pub fn text(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    /// Creates a write with one newline appended, even if the text already ends in one.
    pub fn line(text: impl Into<String>) -> Self {
        let mut text = text.into();
        text.push('\n');
        Self::text(text)
    }

    /// Returns the text that will be written.
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

/// Reads byte chunks from a TCP connection.
///
/// Each subscription makes one connection attempt to an IP address and port.
/// Chunk boundaries do not correspond to application messages; use [`Framed`]
/// with a [`Decoder`] to assemble them. This source does not resolve DNS names,
/// retry, reconnect, or use TLS.
///
/// Return a subscription from your component:
///
/// ```
/// use samara::prelude::*;
/// use std::net::SocketAddr;
/// fn input(
///     tcp: &SourceCapability<TcpBytes>,
///     address: SocketAddr,
/// ) -> Subscriptions<SourceEvent<bytes::Bytes, TcpError>> {
///     Subscriptions::one(Subscription::source(
///         tcp, SubscriptionId::new("feed"), TcpBytes::connect(address),
///     ))
/// }
/// ```
///
/// Declare the capability with `program.source::<TcpBytes>()` and pass it into
/// the component. Enable connections with [`LiveRuntimeBuilder::bind_tcp`].
/// Controlled tests use `control_source::<TcpBytes>()` and
/// [`ControlledRuntime::emit_source`] to supply chunks, errors, or EOF.
/// See [`StdinLines`] for the full source wiring pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TcpBytes {
    endpoint: SocketAddr,
}

impl TcpBytes {
    /// Creates a descriptor for the given IP address and port. It does not connect yet.
    pub fn connect(endpoint: SocketAddr) -> Self {
        Self { endpoint }
    }

    /// Returns the IP address and port.
    pub fn endpoint(&self) -> SocketAddr {
        self.endpoint
    }
}

impl SourceDescriptor for TcpBytes {
    type Item = bytes::Bytes;
    type Error = TcpError;
}

/// The operation that failed while reading a TCP source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TcpErrorKind {
    /// Establishing the single connection failed.
    Connect,
    /// Reading the established byte stream failed.
    Read,
}

/// A TCP connection or read error.
///
/// ```
/// use samara::{TcpError, TcpErrorKind};
/// let error = TcpError::new(TcpErrorKind::Connect, "connection refused");
/// assert_eq!(error.kind(), TcpErrorKind::Connect);
/// assert_eq!(error.message(), "connection refused");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TcpError {
    kind: TcpErrorKind,
    message: Arc<str>,
}

impl TcpError {
    /// Creates a TCP error, for example to simulate a failure in a controlled test.
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

    /// Returns whether connecting or reading failed.
    pub fn kind(&self) -> TcpErrorKind {
        self.kind
    }

    /// Returns the diagnostic message.
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

/// Converts input chunks into application values, optionally buffering incomplete data.
///
/// Use with [`Framed`]. Put configuration on the decoder and per-subscription
/// buffers in `State`. Equality compares configuration to decide whether a source
/// must restart. Methods must be deterministic and must not perform I/O.
///
/// This decoder combines byte chunks into UTF-8 lines:
///
/// ```
/// use samara::prelude::*;
/// #[derive(Clone, Debug, PartialEq)]
/// struct Lines;
/// impl Decoder for Lines {
///     type Chunk = Vec<u8>;
///     type Frame = String;
///     type Error = std::string::FromUtf8Error;
///     type State = Vec<u8>;
///     fn start(&self) -> Self::State { Vec::new() }
///     fn push(&self, buffer: &mut Vec<u8>, chunk: Vec<u8>) -> Result<Vec<String>, Self::Error> {
///         buffer.extend(chunk);
///         let mut lines = Vec::new();
///         while let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
///             let mut line: Vec<u8> = buffer.drain(..=end).collect();
///             line.pop();
///             lines.push(String::from_utf8(line)?);
///         }
///         Ok(lines)
///     }
///     fn finish(&self, buffer: &mut Vec<u8>) -> Result<Vec<String>, Self::Error> {
///         if buffer.is_empty() { return Ok(Vec::new()); }
///         Ok(vec![String::from_utf8(std::mem::take(buffer))?])
///     }
/// }
/// let source = StreamDescriptor::<Vec<u8>>::named("bytes");
/// let mut layer = Framed::new(source, Lines).into_layer();
/// assert!(layer.map_event(SourceEvent::Item(b"hel".to_vec())).is_empty());
/// assert_eq!(layer.map_event(SourceEvent::Item(b"lo\n".to_vec())),
///            vec![SourceEvent::Item("hello".to_owned())]);
/// assert_eq!(layer.map_event(SourceEvent::Ended), vec![SourceEvent::Ended]);
/// ```
pub trait Decoder: Clone + PartialEq + fmt::Debug + Send + Sync + 'static {
    /// An input chunk.
    type Chunk: Send + 'static;
    /// A decoded value.
    type Frame: Send + 'static;
    /// An error that stops decoding.
    type Error: Send + 'static;
    /// One subscription's buffer or other decoding state.
    type State: Send + 'static;

    /// Creates the buffer or other state for one subscription.
    fn start(&self) -> Self::State;

    /// Consumes one chunk and returns any complete values, keeping incomplete data in `state`.
    fn push(
        &self,
        state: &mut Self::State,
        chunk: Self::Chunk,
    ) -> Result<Vec<Self::Frame>, Self::Error>;

    /// Handles normal EOF, returning buffered values or an error for incomplete input.
    ///
    /// Called once on normal source end. It is not called after a source or decoding
    /// error, subscription replacement, cancellation, shutdown, or runtime failure.
    fn finish(&self, state: &mut Self::State) -> Result<Vec<Self::Frame>, Self::Error>;
}

/// A source whose chunks are decoded before delivery to a component.
///
/// Wrap a source with `Framed::new(source, decoder)` and declare a capability for
/// that combined type. Bind or control the underlying source type: for example,
/// `bind_tcp()` or `control_source::<TcpBytes>()` for a framed TCP source.
/// The same decoder runs in live execution and tests.
///
/// See [`Decoder`] for a complete construction and decoding example.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Framed<S, D> {
    /// The source supplying input chunks.
    pub source: S,
    /// The decoder configuration.
    pub decoder: D,
}

impl<S, D> Framed<S, D> {
    /// Combines a source descriptor with a decoder. See [`Decoder`] for an example.
    pub fn new(source: S, decoder: D) -> Self {
        Self { source, decoder }
    }
}

impl<S, D> Framed<S, D>
where
    S: SourceDescriptor<Item = D::Chunk>,
    D: Decoder,
{
    /// Creates a decoder instance for testing without starting the source.
    ///
    /// The runtime creates these instances automatically for subscriptions.
    /// Use [`FramedLayer::map_event`] to supply chunks or EOF directly. See [`Decoder`].
    pub fn into_layer(self) -> FramedLayer<S, D> {
        let state = self.decoder.start();
        FramedLayer {
            descriptor: self,
            state,
        }
    }
}

/// One decoder instance and its buffer, created by [`Framed::into_layer`].
///
/// The runtime creates a separate instance for each subscription. Tests can use
/// it directly to check chunk handling without I/O; see [`Decoder`]. It returns
/// source events without delivering component messages.
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
    /// Returns the wrapped source descriptor, which may itself be another [`Framed`] source.
    pub fn inner_descriptor(&self) -> &S {
        &self.descriptor.source
    }

    /// Decodes one input event into zero or more output events.
    ///
    /// Items pass through [`Decoder::push`]. EOF calls [`Decoder::finish`] and emits
    /// its values followed by `Ended`. A source or decoder error emits `Failed`.
    /// When calling this directly, stop after `Ended` or `Failed`; this method does
    /// not stop the underlying source. See [`Decoder`] for an example.
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

/// An error from either the wrapped source or its decoder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FramedError<SourceError, DecodeError> {
    /// The wrapped input failed.
    Source(SourceError),
    /// The decoder rejected the input.
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

    #[allow(private_interfaces)]
    fn __samara_terminal_type_id(token: source_plan_private::LowerToken) -> TypeId {
        S::__samara_terminal_type_id(token)
    }

    #[allow(private_interfaces)]
    fn __samara_terminal_type_name(token: source_plan_private::LowerToken) -> &'static str {
        S::__samara_terminal_type_name(token)
    }

    #[allow(private_interfaces)]
    fn __samara_terminal_stream_item_type_id(
        token: source_plan_private::LowerToken,
    ) -> Option<TypeId> {
        S::__samara_terminal_stream_item_type_id(token)
    }
}

impl<S, D> stream_source_private::Sealed for Framed<S, D>
where
    S: StreamSourceDescriptor<Item = D::Chunk>,
    D: Decoder,
{
}

impl<S, D> StreamSourceDescriptor for Framed<S, D>
where
    S: StreamSourceDescriptor<Item = D::Chunk>,
    D: Decoder,
{
    type StreamItem = S::StreamItem;
}

impl<S, D> stdin_source_private::Sealed for Framed<S, D>
where
    S: StdinSourceDescriptor<Item = D::Chunk>,
    D: Decoder,
{
}

impl<S, D> StdinSourceDescriptor for Framed<S, D>
where
    S: StdinSourceDescriptor<Item = D::Chunk>,
    D: Decoder,
{
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

/// Permission to request an effect type within a Program.
///
/// Create it with [`ProgramBuilder::effect`], store it on the component, and pass
/// it to [`Command::effect`] or [`Command::effect_with`]. Cloning shares permission
/// within the same program; using it in a different program is an error.
///
/// ```
/// use samara::prelude::*;
/// let mut program = Program::builder();
/// let http = program.effect::<HttpRequest>();
/// let command = Command::effect_with(&http, HttpRequest::get("https://example.test/"), |outcome| outcome);
/// ```
///
/// The runtime builder requires a driver or test control for every declared
/// effect, including effects first requested by later messages.
///
/// <details>
/// <summary>Compile-time checks</summary>
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
/// let _: Command<()> = Command::effect_with(Read, |_| ());
/// ```
///
/// </details>
#[must_use = "store and thread the declared Effect capability into its Component"]
pub struct EffectCapability<D: EffectDescriptor> {
    token: CapabilityToken,
    marker: PhantomData<fn() -> D>,
}

impl<D: EffectDescriptor> Clone for EffectCapability<D> {
    fn clone(&self) -> Self {
        Self {
            token: self.token.clone(),
            marker: PhantomData,
        }
    }
}

impl<D: EffectDescriptor> fmt::Debug for EffectCapability<D> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EffectCapability")
            .field("descriptor", &std::any::type_name::<D>())
            .finish_non_exhaustive()
    }
}

/// Permission to subscribe to a source type within a Program.
///
/// Create it with [`ProgramBuilder::source`] and store it on the component. Pass
/// it to [`Subscription::source`] or [`Subscription::source_with`] together with
/// the source configuration. Cloning does not create a running input, and the
/// capability cannot be used in another Program.
///
/// See [`StdinLines`] for declaration, component storage, subscriptions, and binding.
///
/// <details>
/// <summary>Compile-time checks</summary>
///
/// ```compile_fail
/// use samara::{SourceDescriptor, SourceEvent, Subscription, SubscriptionId};
///
/// #[derive(Clone, Debug, PartialEq)]
/// struct Input;
/// impl SourceDescriptor for Input {
///     type Item = ();
///     type Error = ();
/// }
///
/// let _: Subscription<()> = Subscription::source_with(
///     SubscriptionId::new("input"),
///     Input,
///     |_: SourceEvent<(), ()>| (),
/// );
/// ```
///
/// </details>
#[must_use = "store and thread the declared Source capability into its Component"]
pub struct SourceCapability<S: SourceDescriptor> {
    token: CapabilityToken,
    marker: PhantomData<fn() -> S>,
}

impl<S: SourceDescriptor> Clone for SourceCapability<S> {
    fn clone(&self) -> Self {
        Self {
            token: self.token.clone(),
            marker: PhantomData,
        }
    }
}

impl<S: SourceDescriptor> fmt::Debug for SourceCapability<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourceCapability")
            .field("descriptor", &std::any::type_name::<S>())
            .field(
                "terminal_descriptor",
                &S::__samara_terminal_type_name(source_plan_private::LOWER_TOKEN),
            )
            .finish_non_exhaustive()
    }
}

/// A named connection to a component that implements a [`Protocol`].
///
/// Create it with [`ProgramBuilder::port`] and select its provider with
/// [`ProgramBuilder::bind_port`]. Components use it with [`Command::notify`] and
/// [`Command::request`]. Code outside the program uses a [`PortHandle`] instead.
///
/// ```
/// use samara::prelude::*;
/// protocol! { type Health => enum HealthMessage { Check -> bool, } }
/// let mut program = Program::builder();
/// let health = program.port::<Health>(PortId::new("primary"));
/// assert_eq!(health.id(), &PortId::new("primary"));
/// ```
///
/// A port does not expose the provider's model. See [`ProgramBuilder::bind_port`]
/// for a complete request/reply example.
pub struct Port<P: Protocol> {
    id: PortId,
    program: Arc<()>,
    marker: PhantomData<fn() -> P>,
}

impl<P: Protocol> Port<P> {
    /// Returns the port name.
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

/// Identifies a registered component without giving access to its state.
///
/// Returned by [`ProgramBuilder::component`]. Use it with [`Command::send`] for
/// component-to-component delivery, [`LiveRuntime::handle`] for an external sender,
/// or [`ControlledRuntime::state`] to inspect state in tests.
///
/// See the [crate example](crate) for registration and controlled state inspection.
/// Use a [`Port`] when a caller should depend on a protocol rather than the
/// component's complete message type.
///
/// <details>
/// <summary>Compile-time checks</summary>
///
/// ```compile_fail
/// use samara::{Component, ComponentRef};
///
/// fn bypass_runtime<C: Component>(target: &ComponentRef<C>, message: C::Message) {
///     target.send(message);
/// }
/// ```
///
/// </details>
pub struct ComponentRef<C: Component> {
    id: ComponentId,
    program: Arc<()>,
    marker: PhantomData<fn() -> C>,
}

impl<C: Component> ComponentRef<C> {
    /// Returns the component name.
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

/// Components and their connections, ready to run live or under test control.
///
/// Build one with [`Program::builder`]. Pass it to [`LiveRuntime::builder`] or
/// [`ControlledRuntime::builder`] to supply I/O implementations or test controls.
/// A Program is consumed by its runtime; use a function to build a fresh copy for
/// each run. See the [crate example](crate) for component registration.
///
/// ```
/// use samara::{Program, ControlledRuntime};
/// let program = Program::builder().build()?;
/// let mut runtime = ControlledRuntime::builder(program).build()?;
/// assert_eq!(runtime.run_until_idle()?.transitions, 0);
/// assert!(runtime.cancel()?.is_clean());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct Program {
    program: Arc<()>,
    components: Vec<Box<dyn ErasedComponentKernel>>,
    bindings: Vec<Box<dyn ErasedPortBinding>>,
    effect_requirements: Vec<EffectRequirement>,
    source_requirements: Vec<SourceRequirement>,
}

impl Program {
    /// Starts building a Program. See [`ProgramBuilder`].
    pub fn builder() -> ProgramBuilder {
        let program = Arc::new(());
        ProgramBuilder {
            program,
            components: Vec::new(),
            ports: Vec::new(),
            bindings: Vec::new(),
            effect_requirements: Vec::new(),
            source_requirements: Vec::new(),
            next_capability: 0,
        }
    }
}

pub(crate) struct EffectRequirement {
    token: CapabilityToken,
    descriptor_type: TypeId,
    descriptor_type_name: &'static str,
}

pub(crate) struct SourceRequirement {
    token: CapabilityToken,
    descriptor_type: TypeId,
    descriptor_type_name: &'static str,
    terminal_type: TypeId,
    terminal_type_name: &'static str,
    terminal_stream_item_type: Option<TypeId>,
}

impl Program {
    fn effect_requirement(&self, token: &CapabilityToken) -> Option<&EffectRequirement> {
        self.effect_requirements
            .iter()
            .find(|requirement| requirement.token.same_as(token))
    }

    fn source_requirement(&self, token: &CapabilityToken) -> Option<&SourceRequirement> {
        self.source_requirements
            .iter()
            .find(|requirement| requirement.token.same_as(token))
    }

    pub(crate) fn declares_effect(&self, token: &CapabilityToken, descriptor: TypeId) -> bool {
        token.belongs_to(&self.program)
            && self
                .effect_requirement(token)
                .is_some_and(|requirement| requirement.descriptor_type == descriptor)
    }

    pub(crate) fn declares_source(&self, token: &CapabilityToken, descriptor: TypeId) -> bool {
        token.belongs_to(&self.program)
            && self
                .source_requirement(token)
                .is_some_and(|requirement| requirement.descriptor_type == descriptor)
    }

    pub(crate) fn declares_source_plan(
        &self,
        token: &CapabilityToken,
        descriptor: TypeId,
        terminal: TypeId,
    ) -> bool {
        self.declares_source(token, descriptor)
            && self
                .source_requirement(token)
                .is_some_and(|requirement| requirement.terminal_type == terminal)
    }

    fn declares_component_target(
        &self,
        target_program: &Arc<()>,
        target: &ComponentId,
        message_type: TypeId,
    ) -> bool {
        Arc::ptr_eq(target_program, &self.program)
            && self.components.iter().any(|component| {
                component.id() == target && component.message_type_id() == message_type
            })
    }

    fn declares_port_target(
        &self,
        port_program: &Arc<()>,
        port: &PortId,
        protocol: TypeId,
    ) -> bool {
        Arc::ptr_eq(port_program, &self.program)
            && self
                .bindings
                .iter()
                .any(|binding| binding.port_id() == port && binding.protocol_type() == protocol)
    }
}

pub(crate) fn validate_initial_capabilities<Message>(
    program: &Program,
    component: &ComponentId,
    command: Option<&Command<Message>>,
    subscriptions: &Subscriptions<Message>,
) -> Result<(), RuntimeError>
where
    Message: Send + 'static,
{
    fn validate_command<Message>(
        program: &Program,
        component: &ComponentId,
        command: &Command<Message>,
    ) -> Result<(), RuntimeError>
    where
        Message: Send + 'static,
    {
        match &command.0 {
            CommandKind::None | CommandKind::Reply(_) | CommandKind::After { .. } => Ok(()),
            CommandKind::Effect(effect) => {
                let descriptor_type = effect.intent().type_id();
                if program.declares_effect(effect.capability(), descriptor_type) {
                    Ok(())
                } else {
                    Err(RuntimeError::harness(format!(
                        "Component {component:?} has an initial Effect {} authorized by a capability from another Program",
                        effect.intent_type_name()
                    )))
                }
            }
            CommandKind::Send(send) => {
                if program.declares_component_target(
                    send.target_program(),
                    send.target(),
                    send.message_type_id(),
                ) {
                    Ok(())
                } else {
                    Err(RuntimeError::harness(format!(
                        "Component {component:?} has an initial direct-send target {:?} from another Program or absent from this Program",
                        send.target()
                    )))
                }
            }
            CommandKind::Notify(notification) => {
                if program.declares_port_target(
                    notification.port_program(),
                    notification.port(),
                    notification.protocol_type_id(),
                ) {
                    Ok(())
                } else {
                    Err(RuntimeError::harness(format!(
                        "Component {component:?} has an initial notification Port {:?} from another Program or absent from this Program",
                        notification.port()
                    )))
                }
            }
            CommandKind::Request(request) => {
                if program.declares_port_target(
                    request.port_program(),
                    request.port(),
                    request.protocol_type_id(),
                ) {
                    Ok(())
                } else {
                    Err(RuntimeError::harness(format!(
                        "Component {component:?} has an initial request Port {:?} from another Program or absent from this Program",
                        request.port()
                    )))
                }
            }
            CommandKind::Batch(commands) => {
                for command in commands {
                    validate_command(program, component, command)?;
                }
                Ok(())
            }
        }
    }

    if let Some(command) = command {
        validate_command(program, component, command)?;
    }

    let mut ids = std::collections::HashSet::new();
    for subscription in subscriptions.iter() {
        if !ids.insert(subscription.id().clone()) {
            return Err(RuntimeError::harness(format!(
                "Component {component:?} initially desires duplicate Subscription {:?}",
                subscription.id()
            )));
        }
        let plan = subscription.source_plan();
        if !plan.accepts_output_event_type(subscription.source_event_type_id()) {
            return Err(RuntimeError::harness(format!(
                "Component {component:?} has an invalid initial SourcePlan for {}",
                plan.terminal_type_name()
            )));
        }
        let descriptor_type = subscription.descriptor_any().type_id();
        let Some(requirement) = program.source_requirement(plan.capability()) else {
            return Err(RuntimeError::harness(format!(
                "Component {component:?} has an initial Source {} authorized by a capability from another Program",
                subscription.descriptor_type_name()
            )));
        };
        if !program.declares_source(plan.capability(), descriptor_type)
            || requirement.terminal_type != plan.terminal_type_id()
        {
            return Err(RuntimeError::harness(format!(
                "Component {component:?} has an initial Source capability inconsistent with {}",
                subscription.descriptor_type_name()
            )));
        }
    }
    Ok(())
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

/// A problem with component registration or port wiring.
///
/// Returned by [`ProgramBuilder::build`]. Missing I/O implementations are checked
/// later by the runtime builder. Cyclic port connections are allowed.
///
/// ```
/// use samara::{Program, PortId, Protocol, ProgramBuildError};
/// struct Service;
/// impl Protocol for Service { type Message = (); }
/// let mut builder = Program::builder();
/// let _service = builder.port::<Service>(PortId::new("service"));
/// assert!(matches!(builder.build(), Err(ProgramBuildError::PortBindingCount { count: 0, .. })));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProgramBuildError {
    /// Two components have the same name.
    DuplicateComponentId(ComponentId),
    /// A port name was declared twice for the same protocol.
    DuplicatePort {
        /// The protocol's Rust type name.
        protocol_type: &'static str,
        /// The duplicated port name.
        port: PortId,
    },
    /// A port has no provider or more than one provider.
    PortBindingCount {
        /// The protocol's Rust type name.
        protocol_type: &'static str,
        /// The port name.
        port: PortId,
        /// The number of providers bound to the port.
        count: usize,
    },
    /// A port belongs to another builder.
    ForeignPort {
        /// The protocol's Rust type name.
        protocol_type: &'static str,
        /// The port name.
        port: PortId,
    },
    /// A provider belongs to another builder.
    ForeignProvider {
        /// The provider name.
        provider: ComponentId,
    },
    /// The referenced provider is not registered.
    ProviderNotRegistered {
        /// The provider name.
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

/// Registers components, declares their I/O, and connects ports to providers.
///
/// Start with [`Program::builder`]. Create capabilities before passing them into
/// components. Call [`Self::build`] once wiring is complete, then configure a
/// live or controlled runtime. Capabilities and component references must all
/// come from this builder.
///
/// See the [crate example](crate) for a component, [`StdinLines`] for input, and
/// [`Self::bind_port`] for request/reply wiring.
pub struct ProgramBuilder {
    program: Arc<()>,
    components: Vec<Box<dyn ErasedComponentKernel>>,
    ports: Vec<PortDeclaration>,
    bindings: Vec<Box<dyn ErasedPortBinding>>,
    effect_requirements: Vec<EffectRequirement>,
    source_requirements: Vec<SourceRequirement>,
    next_capability: u64,
}

impl ProgramBuilder {
    fn next_capability(&mut self) -> CapabilityToken {
        let token = CapabilityToken {
            id: self.next_capability,
            program: self.program.clone(),
        };
        self.next_capability += 1;
        token
    }

    /// Declares an effect type and returns the capability used to request it.
    ///
    /// Every declaration needs a driver in live execution or a control in tests,
    /// even if the application has not requested that effect yet.
    ///
    /// ```
    /// use samara::prelude::*;
    /// let mut program = Program::builder();
    /// let http = program.effect::<HttpRequest>();
    /// let runtime = LiveRuntime::builder(program.build()?).bind_http().build()?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn effect<D>(&mut self) -> EffectCapability<D>
    where
        D: EffectDescriptor,
    {
        let token = self.next_capability();
        self.effect_requirements.push(EffectRequirement {
            token: token.clone(),
            descriptor_type: TypeId::of::<D>(),
            descriptor_type_name: std::any::type_name::<D>(),
        });
        EffectCapability {
            token,
            marker: PhantomData,
        }
    }

    /// Declares a source type and returns the capability used to subscribe to it.
    ///
    /// For [`Framed`] sources, bind or control the wrapped input type; decoding still
    /// runs in both live execution and tests. See [`StdinLines`] for an example.
    pub fn source<S>(&mut self) -> SourceCapability<S>
    where
        S: SourceDescriptor,
    {
        let token = self.next_capability();
        self.source_requirements.push(SourceRequirement {
            token: token.clone(),
            descriptor_type: TypeId::of::<S>(),
            descriptor_type_name: std::any::type_name::<S>(),
            terminal_type: S::__samara_terminal_type_id(source_plan_private::LOWER_TOKEN),
            terminal_type_name: S::__samara_terminal_type_name(source_plan_private::LOWER_TOKEN),
            terminal_stream_item_type: S::__samara_terminal_stream_item_type_id(
                source_plan_private::LOWER_TOKEN,
            ),
        });
        SourceCapability {
            token,
            marker: PhantomData,
        }
    }

    /// Registers a component and returns its address.
    ///
    /// Calls [`Component::init`] now and stores the starting model and command.
    /// The command runs when the runtime starts. Component names must be unique;
    /// registration order does not determine live execution order.
    ///
    /// See the [crate example](crate) or [`Self::bind_port`].
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

    /// Declares a named connection to a provider of protocol `P`.
    ///
    /// Bind exactly one provider with [`Self::bind_port`]. Different names allow
    /// several providers of the same protocol. See [`Port`] for construction.
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

    /// Connects a Port to the component that will handle its messages.
    ///
    /// The provider's message type must implement `From<P::Message>`; using the
    /// protocol message type directly also works. Both the port and provider must
    /// belong to this builder. [`Self::build`] checks that every port has exactly one
    /// provider. Cyclic connections are allowed.
    ///
    /// ```
    /// use samara::prelude::*;
    /// protocol! { type Count => enum CountMessage { Read -> u64, } }
    ///
    /// struct Counter;
    /// impl Component for Counter {
    ///     type Model = u64;
    ///     type Message = CountMessage;
    ///     fn init(&self) -> Init<u64, CountMessage> { Init::new(42) }
    ///     fn update(&self, count: &mut u64, message: CountMessage) -> Command<CountMessage> {
    ///         match message {
    ///             CountMessage::Read(call) => Command::reply(call.reply_to, *count),
    ///         }
    ///     }
    /// }
    ///
    /// struct Client { counter: Port<Count> }
    /// impl Component for Client {
    ///     type Model = Option<u64>;
    ///     type Message = RequestOutcome<u64>;
    ///     fn init(&self) -> Init<Self::Model, Self::Message> {
    ///         Init::default().with_command(Command::request(self.counter.clone(), Read))
    ///     }
    ///     fn update(&self, value: &mut Option<u64>, reply: Self::Message) -> Command<Self::Message> {
    ///         if let RequestOutcome::Replied(count) = reply { *value = Some(count); }
    ///         Command::none()
    ///     }
    /// }
    ///
    /// let mut builder = Program::builder();
    /// let port = builder.port::<Count>(PortId::new("counter"));
    /// let counter = builder.component(ComponentId::new("counter"), Counter);
    /// builder.bind_port(&port, &counter);
    /// let client = builder.component(ComponentId::new("client"), Client { counter: port });
    /// let mut runtime = ControlledRuntime::builder(builder.build()?).build()?;
    /// runtime.run_until_idle()?;
    /// assert_eq!(*runtime.state(&client)?, Some(42));
    /// assert!(runtime.cancel()?.is_clean());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
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

    /// Checks component names and port connections and returns the Program.
    ///
    /// Returns [`ProgramBuildError`] for duplicate names, missing or duplicate port
    /// bindings, or references from another builder. Does not start work. I/O drivers
    /// and test controls are checked by the runtime builder.
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
            effect_requirements: self.effect_requirements,
            source_requirements: self.source_requirements,
        })
    }
}

/// Performs the I/O described by an [`EffectDescriptor`] in a live runtime.
///
/// Return the work as a [`BoxFuture`]; Samara runs it, sends its result to the
/// component, and cancels it when needed. Do not mutate component state or spawn
/// detached work. Controlled tests supply results instead of calling the driver.
///
/// This driver checks whether a TCP connection can be established:
///
/// ```
/// use samara::prelude::*;
/// use std::net::SocketAddr;
/// struct CheckConnection(SocketAddr);
/// impl EffectDescriptor for CheckConnection {
///     type Output = ();
///     type Error = std::io::Error;
/// }
/// struct Network;
/// impl EffectDriver<CheckConnection> for Network {
///     fn execute(&self, request: CheckConnection) -> BoxFuture<Result<(), std::io::Error>> {
///         Box::pin(async move {
///             let connection = tokio::net::TcpStream::connect(request.0).await?;
///             drop(connection);
///             Ok(())
///         })
///     }
/// }
/// let mut program = Program::builder();
/// let network = program.effect::<CheckConnection>();
/// let runtime = LiveRuntime::builder(program.build()?)
///     .bind_effect::<CheckConnection, _>(Network).build()?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// The driver may retain resources such as a connection pool. Application state
/// and decisions such as when to retry belong in components or Layers.
pub trait EffectDriver<D: EffectDescriptor>: Send + Sync + 'static {
    /// Returns the future that performs one operation.
    ///
    /// Several calls may be running at once. A successful or failed return supplies
    /// one [`EffectOutcome`]. Cancellation drops the future and response mapper.
    /// A panic while creating or running the future stops the runtime with an error.
    fn execute(&self, descriptor: D) -> BoxFuture<Result<D::Output, D::Error>>;
}

/// Produces events for a source in a live runtime.
///
/// Send items through the supplied [`SourceSink`]. Finish by calling `end` or
/// `fail`, or return normally to end the input. Stop promptly if the sink returns
/// [`DriverStopped`]. Controlled tests supply events without calling the driver.
///
/// ```
/// use samara::prelude::*;
/// use std::convert::Infallible;
/// #[derive(Clone, Debug, PartialEq)]
/// struct Values(Vec<u64>);
/// impl SourceDescriptor for Values {
///     type Item = u64;
///     type Error = Infallible;
/// }
/// struct ValueSource;
/// impl SourceDriver<Values> for ValueSource {
///     fn run(&self, values: Values, sink: SourceSink<Values>) -> BoxFuture<()> {
///         Box::pin(async move {
///             for value in values.0 {
///                 if sink.emit(value).await.is_err() { return; }
///             }
///             let _ = sink.end().await;
///         })
///     }
/// }
/// let mut program = Program::builder();
/// let values = program.source::<Values>();
/// let runtime = LiveRuntime::builder(program.build()?)
///     .bind_source::<Values, _>(ValueSource).build()?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub trait SourceDriver<D: SourceDescriptor>: Send + Sync + 'static {
    /// Returns the future that reads one subscribed input.
    ///
    /// Samara may drop it when the subscription changes or the runtime shuts down.
    /// Keep resources owned by the future so dropping it releases them. Several
    /// subscriptions may run concurrently. A panic stops the runtime with an error.
    fn run(&self, descriptor: D, sink: SourceSink<D>) -> BoxFuture<()>;
}

/// Sends a driver's items, errors, and EOF into the subscribed component.
///
/// Samara creates the sink and passes it to [`SourceDriver::run`]. Events become
/// [`SourceEvent`] values, then messages through the subscription's mapper.
/// The sink does not expose component state. See [`SourceDriver`] for an example.
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

    /// Sends an item and leaves the input open.
    ///
    /// `Ok(())` means the event was accepted, not that the component handled it.
    /// Returns [`DriverStopped`] after input end, failure, removal, or shutdown.
    pub async fn emit(&self, item: D::Item) -> Result<(), DriverStopped> {
        self.inner.send(
            ErasedSourceEvent::typed::<D>(SourceEvent::Item(item)),
            false,
        )
    }

    /// Sends an error and ends the input.
    ///
    /// The first `fail` or `end` call accepted by the runtime succeeds. Later sink
    /// calls return [`DriverStopped`], as do calls after removal or shutdown.
    pub async fn fail(&self, error: D::Error) -> Result<(), DriverStopped> {
        self.inner.send(
            ErasedSourceEvent::typed::<D>(SourceEvent::Failed(error)),
            true,
        )
    }

    /// Sends EOF and ends the input.
    ///
    /// The first `fail` or `end` call accepted by the runtime succeeds. Later sink
    /// calls return [`DriverStopped`], as do calls after removal or shutdown.
    pub async fn end(&self) -> Result<(), DriverStopped> {
        self.inner
            .send(ErasedSourceEvent::typed::<D>(SourceEvent::Ended), true)
    }
}

/// The source no longer accepts events.
///
/// Returned by [`SourceSink`] after the input ends, fails, is removed, or the
/// runtime shuts down. The driver should release its resources and return.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DriverStopped;

impl fmt::Display for DriverStopped {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the runtime stopped the Driver")
    }
}

impl Error for DriverStopped {}

/// An error building, running, or controlling a runtime.
///
/// Use its Display text for a diagnostic. Execution errors can also identify the
/// component, descriptor type, and trace record involved. Setup and test-input
/// errors may lack that context; the accessors then return `None`.
///
/// ```
/// use samara::{ControlledRuntime, HttpRequest, Program};
/// let mut program = Program::builder();
/// let http = program.effect::<HttpRequest>();
/// // The declared HTTP capability needs a test control.
/// let result = ControlledRuntime::builder(program.build()?).build();
/// assert!(result.is_err());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
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

    /// Returns the component involved in an execution error, if known.
    pub fn component(&self) -> Option<&ComponentId> {
        match self.0.as_ref() {
            RuntimeErrorKind::Fault { component, .. } => Some(component),
            RuntimeErrorKind::Harness(_) => None,
        }
    }

    /// Returns the effect or source descriptor type involved, if known.
    pub fn descriptor_type(&self) -> Option<&'static str> {
        match self.0.as_ref() {
            RuntimeErrorKind::Fault {
                descriptor_type, ..
            } => *descriptor_type,
            RuntimeErrorKind::Harness(_) => None,
        }
    }

    /// Returns the trace ID of the work that failed, if known.
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

/// Connects a Program's effects and sources to their I/O implementations.
///
/// Call `bind_*` methods, then [`Self::build`]. Building validates the bindings;
/// [`LiveRuntime::spawn`] starts work. See [`LiveRuntime`] for running a program,
/// [`StdinLines`] for input, and [`EffectDriver`] for a custom driver.
pub struct LiveRuntimeBuilder {
    program: Program,
    bindings: live_runtime::LiveBindings,
}

/// A Program configured to run on Tokio with real I/O.
///
/// Obtain any external senders before calling [`Self::spawn`]. Keep the returned
/// [`RuntimeTask`] to observe errors and wait for shutdown.
///
/// ```
/// use samara::prelude::*;
/// struct Counter;
/// impl Component for Counter {
///     type Model = u64;
///     type Message = u64;
///     fn init(&self) -> Init<u64, u64> { Init::new(0) }
///     fn update(&self, count: &mut u64, amount: u64) -> Command<u64> {
///         *count += amount;
///         Command::none()
///     }
/// }
/// let tokio = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
/// tokio.block_on(async {
///     let mut builder = Program::builder();
///     let counter = builder.component(ComponentId::new("counter"), Counter);
///     let runtime = LiveRuntime::builder(builder.build()?).build()?;
///     let sender = runtime.handle(&counter)?;
///     let task = runtime.spawn();
///     sender.send(3).await?;
///     // Drain processes the accepted message before returning.
///     assert!(task.shutdown(Shutdown::Drain).await?.is_clean());
///     Ok::<(), Box<dyn std::error::Error>>(())
/// })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// Updates to one component never overlap. Independent events can arrive in
/// either order. Internal message queues have no size limit, so sustained overload
/// can grow memory use. Cancellation and runtime errors may discard queued work.
pub struct LiveRuntime {
    program: Program,
    bindings: live_runtime::LiveBindings,
    scope: Arc<live_runtime::LiveScope>,
    receiver: tokio::sync::mpsc::UnboundedReceiver<live_runtime::LiveEvent>,
}

impl LiveRuntime {
    /// Starts configuring I/O for a Program. See [`LiveRuntime`].
    pub fn builder(program: Program) -> LiveRuntimeBuilder {
        LiveRuntimeBuilder {
            program,
            bindings: live_runtime::LiveBindings::new(),
        }
    }

    /// Creates a sender for code outside the Samara program.
    ///
    /// Returns an error if the component does not belong to this Program. Keep the
    /// handle in host code; components send via Commands instead. See [`LiveRuntime`].
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

    /// Creates a protocol client for code outside the Samara program.
    ///
    /// The port must be declared and bound in this Program. Otherwise returns an
    /// error. Keep the handle in host code; components use [`Port`] and Commands.
    /// See [`PortHandle`] for an example.
    pub fn port_handle<P: Protocol>(&self, port: &Port<P>) -> Result<PortHandle<P>, RuntimeError> {
        if !Arc::ptr_eq(&port.program, &self.program.program)
            || !self.program.bindings.iter().any(|binding| {
                binding.protocol_type() == TypeId::of::<P>()
                    && binding.port_id() == port.id()
                    && Arc::ptr_eq(binding.port_program(), &port.program)
            })
        {
            return Err(RuntimeError::harness(
                "live Port handle target is not part of this Program",
            ));
        }
        Ok(PortHandle {
            port: port.clone(),
            scope: self.scope.clone(),
        })
    }

    /// Starts the program on the current Tokio runtime.
    ///
    /// Returns a [`RuntimeTask`] that owns the running work. Await its shutdown to
    /// wait for resource cleanup. See [`LiveRuntime`] for a complete example.
    ///
    /// # Panics
    ///
    /// Panics if called outside a Tokio runtime.
    pub fn spawn(self) -> RuntimeTask {
        let scope = self.scope.clone();
        let core =
            live_runtime::LiveCore::new(self.program, self.bindings, self.scope, self.receiver);
        RuntimeTask {
            scope,
            join: Some(tokio::spawn(core.run())),
            completion: None,
        }
    }
}

impl LiveRuntimeBuilder {
    /// Connects a source capability to a Tokio receiver.
    ///
    /// Use a capability for [`StreamDescriptor`] or a [`Framed`] source wrapping one.
    /// The receiver is consumed by its first subscription and cannot be restarted.
    /// When the channel closes, the source emits [`SourceEvent::Ended`].
    /// See [`StreamDescriptor`] for construction.
    pub fn bind_mpsc<S>(
        mut self,
        stream: &SourceCapability<S>,
        receiver: tokio::sync::mpsc::Receiver<S::StreamItem>,
    ) -> Self
    where
        S: StreamSourceDescriptor,
    {
        live_runtime::bind_mpsc(&mut self.bindings, stream, receiver);
        self
    }

    /// Connects a source capability to process standard input on Unix.
    ///
    /// Accepts [`StdinLines`] or a [`Framed`] source wrapping it. Only one active
    /// subscription may read stdin across all runtimes; a competing reader faults
    /// the runtime. Do not also read stdin outside Samara.
    ///
    /// Removing the subscription or shutting down interrupts the read without waiting
    /// for another byte. A later subscription resumes at stdin's current position.
    /// See [`StdinLines`] for a complete example.
    #[cfg(unix)]
    pub fn bind_stdin<S>(mut self, stdin: &SourceCapability<S>) -> Self
    where
        S: StdinSourceDescriptor,
    {
        live_runtime::bind_stdin(&mut self.bindings, stdin);
        self
    }

    /// Registers an implementation of effect type `D`.
    ///
    /// Register at most one driver per effect type; duplicates fail at [`Self::build`].
    /// See [`EffectDriver`] for an implementation and registration example.
    pub fn bind_effect<D, Driver>(mut self, driver: Driver) -> Self
    where
        D: EffectDescriptor,
        Driver: EffectDriver<D>,
    {
        self.bindings.bind_effect::<D, Driver>(driver);
        self
    }

    /// Registers an implementation of source type `D`.
    ///
    /// For a [`Framed`] source, register the wrapped source's driver. Samara runs the
    /// decoder before delivering messages. Duplicate or conflicting bindings fail
    /// at [`Self::build`]. See [`SourceDriver`] for an example.
    pub fn bind_source<D, Driver>(mut self, driver: Driver) -> Self
    where
        D: SourceDescriptor,
        Driver: SourceDriver<D>,
    {
        self.bindings.bind_source::<D, Driver>(driver);
        self
    }

    /// Enables [`TcpBytes`] subscriptions using Tokio TCP connections.
    ///
    /// Each input connects once to the configured IP address and port. No DNS, retry,
    /// or reconnect is performed. See [`TcpBytes`] for construction.
    pub fn bind_tcp(mut self) -> Self {
        live_runtime::bind_tcp(&mut self.bindings);
        self
    }

    /// Enables [`HttpRequest`] effects with a shared HTTP client and connection pool.
    ///
    /// ```
    /// use samara::prelude::*;
    /// let mut program = Program::builder();
    /// let http = program.effect::<HttpRequest>();
    /// let runtime = LiveRuntime::builder(program.build()?).bind_http().build()?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// The driver makes one request, buffers the response, and returns any HTTP status.
    /// It does not retry, follow redirects, use system proxies, or decompress bodies.
    /// Use [`HttpResponsePipeline`] for redirect, status, and JSON handling.
    /// There is no request timeout or body-size limit. `Accept: */*` is supplied if
    /// no Accept header was set. Controlled tests use `control_effect::<HttpRequest>()`.
    pub fn bind_http(mut self) -> Self {
        live_runtime::bind_http(&mut self.bindings);
        self
    }

    /// Enables [`PrintStdout`] and [`PrintStderr`] effects.
    ///
    /// Each write attempts to send all text and flush the stream. I/O errors are
    /// ignored. Writes to a stream do not interleave with each other through this
    /// binding, but independent commands have no promised completion order. Direct
    /// process writes and writes to different streams are not ordered by Samara.
    ///
    /// ```
    /// use samara::prelude::*;
    /// let mut program = Program::builder();
    /// let output = program.effect::<PrintStdout>();
    /// let runtime = LiveRuntime::builder(program.build()?).bind_stdio().build()?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn bind_stdio(mut self) -> Self {
        live_runtime::bind_stdio(&mut self.bindings);
        self
    }

    /// Checks the bindings and returns a runtime ready to start.
    ///
    /// Every declared effect and source needs one matching implementation, even if
    /// unused at startup. Missing, duplicate, conflicting, or foreign bindings return
    /// an error, as do foreign capabilities in startup commands and subscriptions.
    /// Call [`LiveRuntime::spawn`] to begin work.
    pub fn build(self) -> Result<LiveRuntime, RuntimeError> {
        self.bindings.validate(&self.program)?;
        for component in &self.program.components {
            component.validate_initial_capabilities(&self.program)?;
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

/// Sends messages to a component from surrounding async code.
///
/// Create it with [`LiveRuntime::handle`] before spawning the runtime. Clones
/// send to the same component. The handle does not expose its model and should
/// not be stored in a component. See [`LiveRuntime`] for a working example.
///
/// <details>
/// <summary>Compile-time checks</summary>
///
/// ```compile_fail
/// use samara::{Component, ComponentHandle};
///
/// fn replace_model<C: Component>(handle: &mut ComponentHandle<C>, model: C::Model) {
///     *handle.model_mut() = model;
/// }
/// ```
///
/// </details>
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
    /// Queues a message for this component.
    ///
    /// Success means the message was accepted, not that `update` has run. Returns an
    /// error after shutdown begins or the runtime fails. See [`LiveRuntime`].
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

/// Calls a protocol from async code outside a Samara program.
///
/// Create it with [`LiveRuntime::port_handle`] before spawning the runtime.
/// Clones use the same port. Notifications and requests still go through the
/// provider's `update` method; the handle does not expose its model.
///
/// ```
/// use samara::prelude::*;
/// protocol! { type Count => enum CountMessage { Read -> u64, } }
/// struct Counter;
/// impl Component for Counter {
///     type Model = u64;
///     type Message = CountMessage;
///     fn init(&self) -> Init<u64, CountMessage> { Init::new(42) }
///     fn update(&self, count: &mut u64, message: CountMessage) -> Command<CountMessage> {
///         match message {
///             CountMessage::Read(call) => Command::reply(call.reply_to, *count),
///         }
///     }
/// }
/// let tokio = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
/// tokio.block_on(async {
///     let mut builder = Program::builder();
///     let count = builder.port::<Count>(PortId::new("count"));
///     let counter = builder.component(ComponentId::new("counter"), Counter);
///     builder.bind_port(&count, &counter);
///     let runtime = LiveRuntime::builder(builder.build()?).build()?;
///     let client = runtime.port_handle(&count)?;
///     let task = runtime.spawn();
///     assert_eq!(client.request(Read).await?, 42);
///     assert!(task.shutdown(Shutdown::Drain).await?.is_clean());
///     Ok::<(), Box<dyn std::error::Error>>(())
/// })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// Components use [`Port`] and Commands instead of storing these handles.
///
/// <details>
/// <summary>Compile-time checks</summary>
///
/// ```compile_fail
/// use samara::{PortHandle, Protocol};
///
/// fn mutate_provider<P: Protocol>(handle: &mut PortHandle<P>) {
///     handle.model_mut();
/// }
/// ```
///
/// ```compile_fail
/// use samara::{Command, Notification, PortHandle, Protocol};
///
/// struct ExampleProtocol;
/// enum ExampleProtocolMessage { Notified }
/// struct Notify;
///
/// impl Protocol for ExampleProtocol {
///     type Message = ExampleProtocolMessage;
/// }
///
/// impl Notification<ExampleProtocol> for Notify {
///     fn into_message(self) -> ExampleProtocolMessage {
///         ExampleProtocolMessage::Notified
///     }
/// }
///
/// fn misuse(handle: PortHandle<ExampleProtocol>) -> Command<()> {
///     Command::notify(handle, Notify)
/// }
/// ```
///
/// </details>
pub struct PortHandle<P: Protocol> {
    port: Port<P>,
    scope: Arc<live_runtime::LiveScope>,
}

impl<P: Protocol> Clone for PortHandle<P> {
    fn clone(&self) -> Self {
        Self {
            port: self.port.clone(),
            scope: self.scope.clone(),
        }
    }
}

impl<P: Protocol> fmt::Debug for PortHandle<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("PortHandle")
            .field(self.port.id())
            .finish()
    }
}

impl<P: Protocol> PortHandle<P> {
    /// Queues a notification for the provider.
    ///
    /// Success means accepted for delivery, not handled by the provider. Returns an
    /// error if shutdown has begun or the runtime has failed.
    ///
    /// ```no_run
    /// use samara::prelude::*;
    /// protocol! { type Events => enum EventMessage { Changed(String), } }
    /// async fn notify(client: &PortHandle<Events>) -> Result<(), RuntimeError> {
    ///     client.notify(Changed("ready".into())).await
    /// }
    /// ```
    pub async fn notify<N>(&self, notification: N) -> Result<(), RuntimeError>
    where
        N: Notification<P>,
    {
        self.scope
            .accept_ingress(live_runtime::LiveEvent::port_notification(
                self.port.clone(),
                notification,
            ))
    }

    /// Sends a request and waits for its reply without a timeout.
    ///
    /// Dropping this future after sending stops waiting but does not cancel the
    /// provider's work or release the pending request. An unanswered request can
    /// keep Drain waiting. Runtime shutdown or failure before the reply returns
    /// [`RuntimeError`]. See [`PortHandle`] for an example.
    pub async fn request<R>(&self, request: R) -> Result<R::Reply, RuntimeError>
    where
        R: Request<P>,
    {
        match self.request_inner(request, None).await? {
            RequestOutcome::Replied(reply) => Ok(reply),
            _ => unreachable!("unbounded host requests only complete with a reply"),
        }
    }

    /// Sends a request and waits for its reply until `timeout` expires.
    ///
    /// The clock starts when the future is first polled, including time waiting in
    /// the runtime's queue. A reply processed at or after the deadline produces
    /// [`RequestOutcome::TimedOut`]. Zero always times out. The provider keeps working;
    /// later replies are ignored. Dropping this future does not remove the timeout.
    /// Shutdown, runtime failure, or clock overflow returns [`RuntimeError`].
    ///
    /// ```no_run
    /// use samara::prelude::*;
    /// use std::time::Duration;
    /// protocol! { type Health => enum HealthMessage { Check -> bool, } }
    /// async fn check(client: &PortHandle<Health>) -> Result<RequestOutcome<bool>, RuntimeError> {
    ///     client.request_timeout(Check, Duration::from_secs(2)).await
    /// }
    /// ```
    pub async fn request_timeout<R>(
        &self,
        request: R,
        timeout: Duration,
    ) -> Result<RequestOutcome<R::Reply>, RuntimeError>
    where
        R: Request<P>,
    {
        self.request_inner(request, Some(timeout)).await
    }

    async fn request_inner<R>(
        &self,
        request: R,
        timeout: Option<Duration>,
    ) -> Result<RequestOutcome<R::Reply>, RuntimeError>
    where
        R: Request<P>,
    {
        let deadline = timeout
            .map(|timeout| {
                tokio::time::Instant::now()
                    .checked_add(timeout)
                    .ok_or_else(|| RuntimeError::harness("request deadline is unrepresentable"))
            })
            .transpose()?;
        let (completion, reply) = tokio::sync::oneshot::channel();
        self.scope
            .accept_ingress(live_runtime::LiveEvent::port_request(
                self.port.clone(),
                request,
                deadline,
                completion,
            ))?;
        reply.await.map_err(|_| self.scope.request_ended_error())
    }
}

/// Owns a running Samara program and provides shutdown and error reporting.
///
/// Returned by [`LiveRuntime::spawn`]. Await [`Self::shutdown`] to stop the program
/// and wait for cleanup. Use [`Self::run_forever`] to monitor it while waiting for
/// an external shutdown signal.
///
/// ```
/// use samara::prelude::*;
/// let tokio = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
/// tokio.block_on(async {
///     let program = Program::builder().build()?;
///     let task = LiveRuntime::builder(program).build()?.spawn();
///     assert!(task.shutdown(Shutdown::Drain).await?.is_clean());
///     Ok::<(), Box<dyn std::error::Error>>(())
/// })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// Dropping the task requests cancellation and aborts Samara's tasks without
/// waiting for cleanup. Prefer an awaited shutdown when resource release matters.
/// [`Self::request_shutdown`] shows how to give Drain a grace period before Cancel.
pub struct RuntimeTask {
    scope: Arc<live_runtime::LiveScope>,
    join: Option<tokio::task::JoinHandle<Result<ShutdownReport, RuntimeError>>>,
    completion: Option<Result<ShutdownReport, RuntimeError>>,
}

impl RuntimeTask {
    /// Starts shutdown without waiting for it to finish.
    ///
    /// Closes external message delivery before returning. A later Cancel can escalate
    /// Drain; a later Drain cannot reverse Cancel. Repeated calls are harmless and
    /// do not replace an already recorded error or shutdown result.
    ///
    /// Wait for cleanup with [`Self::run_forever`] or [`Self::shutdown`]. To allow a
    /// grace period, wait for Drain and escalate if it takes too long:
    ///
    /// ```no_run
    /// use samara::{RuntimeError, RuntimeTask, Shutdown, ShutdownReport};
    /// use std::time::Duration;
    /// async fn stop(mut task: RuntimeTask) -> Result<ShutdownReport, RuntimeError> {
    ///     task.request_shutdown(Shutdown::Drain);
    ///     match tokio::time::timeout(Duration::from_secs(2), task.run_forever()).await {
    ///         Ok(result) => result,
    ///         Err(_) => {
    ///             task.request_shutdown(Shutdown::Cancel);
    ///             task.run_forever().await
    ///         }
    ///     }
    /// }
    /// ```
    ///
    /// The grace period limits waiting for Drain, not the time needed for cleanup.
    /// Cancelling the wait does not undo the shutdown request.
    pub fn request_shutdown(&mut self, mode: Shutdown) {
        // A fault is preserved in the scope, and a closed event channel means
        // the owner has ended. Its retained JoinHandle supplies the terminal
        // result in both cases, so request submission needs no second error
        // channel that could bypass awaiting cleanup.
        let _ = self.scope.begin_shutdown(mode);
    }

    /// Waits for the program to stop and returns its shutdown report or error.
    ///
    /// Does not initiate shutdown. A running program keeps this future pending even
    /// when it currently has no work. Dropping the future leaves the RuntimeTask
    /// owning the program and preserves any earlier shutdown request.
    ///
    /// The host can supply a Ctrl-C handler or another shutdown future:
    ///
    /// ```no_run
    /// use samara::{RuntimeError, RuntimeTask, Shutdown, ShutdownReport};
    /// async fn run_until(
    ///     mut task: RuntimeTask,
    ///     stop: impl std::future::Future<Output = ()>,
    /// ) -> Result<ShutdownReport, RuntimeError> {
    ///     tokio::select! {
    ///         result = task.run_forever() => return result,
    ///         () = stop => {}
    ///     }
    ///     task.shutdown(Shutdown::Cancel).await
    /// }
    /// ```
    pub async fn run_forever(&mut self) -> Result<ShutdownReport, RuntimeError> {
        self.await_completion().await
    }

    /// Stops the program and waits for all Samara work to be cleaned up.
    ///
    /// Returns a report, or the runtime error if execution failed. An earlier Drain
    /// can be escalated to Cancel, but Cancel cannot be reversed. If a result was
    /// already observed, returns the same result. Consumes the task; cancelling this
    /// future drops the task and requests cancellation.
    ///
    /// See [`RuntimeTask`] for an example, or [`Self::request_shutdown`] to start
    /// shutdown while keeping the task for later observation.
    pub async fn shutdown(mut self, mode: Shutdown) -> Result<ShutdownReport, RuntimeError> {
        self.request_shutdown(mode);
        self.await_completion().await
    }

    async fn await_completion(&mut self) -> Result<ShutdownReport, RuntimeError> {
        if let Some(completion) = &self.completion {
            return completion.clone();
        }

        let completion = self
            .join
            .as_mut()
            .expect("an unobserved RuntimeTask owns one live runtime task")
            .await
            .map_err(|_| RuntimeError::harness("live runtime owner task panicked"))
            .and_then(|result| result);

        // Keep the JoinHandle inside `self` across the await. If a host select,
        // timeout, or cancelled shutdown future drops this observation,
        // `RuntimeTask::drop` can still abort the owner instead of detaching it.
        // Cache a completed result so later observation preserves the original
        // diagnostic without polling a completed JoinHandle again.
        self.join.take();
        self.completion = Some(completion.clone());
        completion
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

/// How a live runtime should stop.
///
/// Use [`Self::Drain`] to finish accepted work or [`Self::Cancel`] to abort it.
/// Both reject new external messages and stop sources. See
/// [`RuntimeTask::request_shutdown`] for escalation from Drain to Cancel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shutdown {
    /// Finishes accepted messages, effects, timers, and requests, including work
    /// created while processing them. Stops sources and starts no new ones.
    ///
    /// There is no deadline. Recurring commands, unanswered requests, or hung I/O
    /// can keep Drain waiting indefinitely.
    Drain,
    /// Stops processing messages and aborts outstanding work.
    ///
    /// Does not send cancellation messages to components just because the runtime
    /// is stopping. Cleanup is still awaited.
    Cancel,
}

/// Counts of completed, cancelled, and remaining work after shutdown.
///
/// Successful shutdown guarantees zero remaining work. The completed and
/// cancelled counts are diagnostics; their exact values can change between
/// versions. See [`RuntimeTask`] for an example using [`Self::is_clean`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShutdownReport {
    /// Work completed during shutdown.
    pub completed: usize,
    /// Work cancelled during shutdown.
    pub cancelled: usize,
    /// Work still pending after shutdown.
    pub remaining: usize,
    /// Ready work still pending after shutdown.
    pub pending_now: usize,
    /// Work still waiting for time or input after shutdown.
    pub pending_later: usize,
}

impl ShutdownReport {
    /// Returns whether no work remains (`remaining == 0`).
    pub fn is_clean(&self) -> bool {
        self.remaining == 0
    }
}

/// Selects the effects and sources a test will control.
///
/// Every declared effect or source needs a matching control. Missing controls
/// are errors, never a fallback to live I/O. See [`ControlledRuntime`] for effects
/// and [`StdinLines`] for source events.
pub struct ControlledRuntimeBuilder {
    program: Program,
    bindings: controlled_runtime::ControlledBindings,
}

/// Runs a Program synchronously with inputs, effect results, and time supplied by a test.
///
/// No drivers run. Your components, response handling, and decoders are the same
/// as in live execution. Drive ready work with [`Self::run_until_idle`], inspect
/// state, and provide the next input or result. Identical inputs and clock advances
/// produce identical traces.
///
/// ```
/// use samara::prelude::*;
/// struct Check { http: EffectCapability<HttpRequest> }
/// impl Component for Check {
///     type Model = Option<StatusCode>;
///     type Message = EffectOutcome<HttpResponse, HttpError>;
///     fn init(&self) -> Init<Self::Model, Self::Message> {
///         Init::default().with_command(Command::effect(
///             &self.http, HttpRequest::get("https://example.test/health"),
///         ))
///     }
///     fn update(&self, status: &mut Self::Model, outcome: Self::Message) -> Command<Self::Message> {
///         if let EffectOutcome::Succeeded(response) = outcome {
///             *status = Some(response.status());
///         }
///         Command::none()
///     }
/// }
/// let mut builder = Program::builder();
/// let http = builder.effect::<HttpRequest>();
/// let check = builder.component(ComponentId::new("check"), Check { http });
/// let mut runtime = ControlledRuntime::builder(builder.build()?)
///     .control_effect::<HttpRequest>().build()?;
/// runtime.run_until_idle()?;
/// let request = runtime.next_effect::<HttpRequest>()?;
/// assert_eq!(request.intent.url(), "https://example.test/health");
/// let response = HttpResponse::new(StatusCode::OK, http::Version::HTTP_11, http::HeaderMap::new(), "");
/// runtime.complete(request, EffectOutcome::Succeeded(response))?;
/// runtime.run_until_idle()?;
/// assert_eq!(*runtime.state(&check)?, Some(StatusCode::OK));
/// assert!(runtime.cancel()?.is_clean());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// See [`StdinLines`] for source inputs and [`Self::advance`] for timers.
pub struct ControlledRuntime {
    core: ControlledCore,
}

impl ControlledRuntime {
    /// Starts configuring a test runtime for this Program. See [`ControlledRuntime`].
    pub fn builder(program: Program) -> ControlledRuntimeBuilder {
        ControlledRuntimeBuilder {
            program,
            bindings: controlled_runtime::ControlledBindings::new(),
        }
    }

    /// Queues a message for a component without running its update yet.
    ///
    /// Call [`Self::run_until_idle`] or a clock-advancing method to process it.
    /// Returns an error for a component from another Program. See the [crate example](crate).
    pub fn send<C: Component>(
        &mut self,
        component: &ComponentRef<C>,
        message: C::Message,
    ) -> Result<(), RuntimeError> {
        self.core.send(component, message)
    }

    /// Supplies a channel item to the subscription using this capability.
    ///
    /// For a framed stream, provide the original chunk; Samara runs the decoder.
    /// The subscription must be active and the capability must belong to this Program.
    /// Call [`Self::run_until_idle`] to deliver the resulting messages.
    /// See [`StreamDescriptor`] for an example.
    pub fn emit_stream<S>(
        &mut self,
        stream: &SourceCapability<S>,
        item: S::StreamItem,
    ) -> Result<(), RuntimeError>
    where
        S: StreamSourceDescriptor,
    {
        self.core.emit_stream(stream, item)
    }

    /// Supplies EOF to the subscription using this capability.
    ///
    /// The subscription must be active. Decoders process EOF before the subscription
    /// mapper receives [`SourceEvent::Ended`]. Call [`Self::run_until_idle`] to deliver
    /// messages. See [`StreamDescriptor`] for an example.
    pub fn close_stream<S: StreamSourceDescriptor>(
        &mut self,
        stream: &SourceCapability<S>,
    ) -> Result<(), RuntimeError> {
        self.core.close_stream(stream)
    }

    /// Supplies an event to a component's active subscription.
    ///
    /// Use the underlying input type for `S`: for `Framed<TcpBytes, D>`, provide
    /// `SourceEvent<bytes::Bytes, TcpError>` with `S = TcpBytes`. Samara runs the decoder
    /// before the message mapper. An inactive subscription, wrong type, or component
    /// from another Program returns an error.
    ///
    /// Call [`Self::run_until_idle`] to process the input. See [`StdinLines`] for a
    /// complete example.
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

    /// Returns a clone of the active subscription's descriptor.
    ///
    /// Use the full descriptor type for `S`, including any [`Framed`] wrappers.
    /// Returns an error if the component, subscription, or descriptor type does not
    /// match. This inspects configuration, not an open connection or driver state.
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

    /// Takes the next pending effect of type `E` for inspection and completion.
    ///
    /// Call [`Self::run_until_idle`] first to issue startup commands or process
    /// messages. Returns an error if no unclaimed effect of that type exists.
    /// An effect can be taken only once and remains pending until [`Self::complete`]
    /// or [`Self::cancel`], even if its [`PendingEffect`] is dropped.
    ///
    /// Selection order is repeatable in tests; it does not promise live completion
    /// order. See [`ControlledRuntime`] for a complete example.
    pub fn next_effect<E: EffectDescriptor>(&mut self) -> Result<PendingEffect<E>, RuntimeError> {
        self.core.next_effect()
    }

    /// Supplies a result for an effect taken from this runtime.
    ///
    /// Runs its mapper and queues the resulting message. Redirect-following pipelines
    /// may issue another HTTP request instead; effects that discard results queue no
    /// message. No driver or I/O runs. Call [`Self::run_until_idle`] to handle any
    /// queued messages. See [`ControlledRuntime`] for an example.
    ///
    /// Returns an error if the effect belongs to another runtime or is no longer
    /// pending.
    pub fn complete<E: EffectDescriptor>(
        &mut self,
        pending: PendingEffect<E>,
        outcome: EffectOutcome<E::Output, E::Error>,
    ) -> Result<(), RuntimeError> {
        self.core.complete(pending, outcome)
    }

    /// Processes ready work until only future timers and external inputs remain.
    ///
    /// Does not advance time or supply effect results. Recurring immediate commands
    /// can prevent it from returning. Returns the number of updates and remaining
    /// work counts. See [`ControlledRuntime`] for an example.
    pub fn run_until_idle(&mut self) -> Result<RunReport, RuntimeError> {
        self.core.run_until_idle()
    }

    /// Moves the test clock forward by `duration` and processes work along the way.
    ///
    /// Processes work ready now before moving time, then visits each due deadline
    /// through the requested time. Does not sleep or supply effect results. Clock
    /// overflow returns an error.
    ///
    /// ```
    /// use samara::prelude::*;
    /// use std::time::Duration;
    /// struct Timer;
    /// impl Component for Timer {
    ///     type Model = bool;
    ///     type Message = ();
    ///     fn init(&self) -> Init<bool, ()> {
    ///         Init::new(false).with_command(Command::after(Duration::from_secs(2), ()))
    ///     }
    ///     fn update(&self, fired: &mut bool, _: ()) -> Command<()> {
    ///         *fired = true;
    ///         Command::none()
    ///     }
    /// }
    /// let mut builder = Program::builder();
    /// let timer = builder.component(ComponentId::new("timer"), Timer);
    /// let mut runtime = ControlledRuntime::builder(builder.build()?).build()?;
    /// runtime.advance(Duration::from_secs(1))?;
    /// assert!(!*runtime.state(&timer)?);
    /// runtime.advance_to_next()?;
    /// assert!(*runtime.state(&timer)?);
    /// assert_eq!(runtime.trace().last().unwrap().at.as_duration(), Duration::from_secs(2));
    /// runtime.cancel()?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn advance(&mut self, duration: Duration) -> Result<RunReport, RuntimeError> {
        self.core.advance(duration)
    }

    /// Moves the test clock to the next scheduled deadline and processes ready work.
    ///
    /// Returns an error if no future work is scheduled. Does not supply effect results.
    /// See [`Self::advance`] for an example.
    pub fn advance_to_next(&mut self) -> Result<RunReport, RuntimeError> {
        self.core.advance_to_next()
    }

    /// Stops the test runtime and cancels its remaining work.
    ///
    /// Consumes the runtime and returns a report with no work remaining. It does not
    /// send cancellation messages to components. To test a component's cancellation
    /// handling, supply an [`EffectOutcome::Cancelled`] to [`Self::complete`] first.
    /// See [`ControlledRuntime`] for cleanup after a test.
    pub fn cancel(self) -> Result<ShutdownReport, RuntimeError> {
        self.core.cancel()
    }

    /// Borrows a component's model for assertions without running any work.
    ///
    /// Returns an error if the component does not belong to this Program. Live
    /// runtimes do not expose models. See [`ControlledRuntime`] for an example.
    pub fn state<C: Component>(
        &self,
        component: &ComponentRef<C>,
    ) -> Result<&C::Model, RuntimeError> {
        self.core.state(component)
    }

    /// Returns counts of ready and waiting work without processing it.
    pub fn pending_work(&self) -> PendingWork {
        self.core.pending_work()
    }

    /// Returns the recorded events, their test-clock times, and causal links.
    ///
    /// Reading the trace does not run work or affect components. It records event
    /// kinds and Rust type names, not message or descriptor payloads. See
    /// [`TraceRecord`] for an example of inspecting it.
    pub fn trace(&self) -> &[TraceRecord] {
        self.core.trace()
    }
}

impl ControlledRuntimeBuilder {
    /// Lets a test supply events for this channel capability.
    ///
    /// Use [`ControlledRuntime::emit_stream`] and [`ControlledRuntime::close_stream`].
    /// Accepts a plain [`StreamDescriptor`] or a [`Framed`] source wrapping it.
    /// Registering the same capability twice, mixing this with type-wide control of
    /// its input, or using a foreign capability fails at [`Self::build`].
    /// See [`StreamDescriptor`] for a complete example.
    pub fn control_stream<S: StreamSourceDescriptor>(
        mut self,
        stream: &SourceCapability<S>,
    ) -> Self {
        self.bindings.exact_sources.push(stream.token.clone());
        self
    }

    /// Lets a test inspect requests and supply results for effect type `E`.
    ///
    /// Use [`ControlledRuntime::next_effect`] and [`ControlledRuntime::complete`].
    /// No live driver runs. See [`ControlledRuntime`] for a complete example.
    pub fn control_effect<E: EffectDescriptor>(mut self) -> Self {
        self.bindings.effects.insert(TypeId::of::<E>());
        self
    }

    /// Lets a test supply events for source type `S`.
    ///
    /// For a [`Framed`] source, register the wrapped input type, so the decoder stays
    /// part of the test. Supply events with [`ControlledRuntime::emit_source`].
    /// See [`StdinLines`] for a complete example.
    pub fn control_source<S: SourceDescriptor>(mut self) -> Self {
        self.bindings.sources.insert(TypeId::of::<S>());
        self
    }

    /// Checks test controls and returns a runtime ready to drive.
    ///
    /// Every declared effect and source needs one matching control, even if unused
    /// at startup. Missing, conflicting, foreign, or incompatible controls return an
    /// error, as do foreign capabilities in startup work. Call
    /// [`ControlledRuntime::run_until_idle`] to process startup work.
    pub fn build(self) -> Result<ControlledRuntime, RuntimeError> {
        self.bindings.validate(&self.program)?;
        for component in &self.program.components {
            component.validate_initial_capabilities(&self.program)?;
        }
        Ok(ControlledRuntime {
            core: ControlledCore::new(self.program, self.bindings),
        })
    }
}

/// An effect taken by a test and awaiting its supplied result.
///
/// Inspect `intent`, then pass the whole value to [`ControlledRuntime::complete`]
/// on the same runtime. Dropping it does not cancel or complete the effect.
/// See [`ControlledRuntime`] for a complete example.
#[must_use = "a PendingEffect remains outstanding until completed or cancelled"]
pub struct PendingEffect<E: EffectDescriptor> {
    /// The effect arguments to inspect before supplying a result.
    pub intent: E,
    id: u64,
    program: Arc<()>,
}

/// Counts of ready work and work waiting for time or input.
///
/// Returned by [`ControlledRuntime::pending_work`]. Inspecting these counts does
/// not process work.
///
/// ```
/// use samara::{Program, ControlledRuntime};
/// let mut runtime = ControlledRuntime::builder(Program::builder().build()?).build()?;
/// runtime.run_until_idle()?;
/// let work = runtime.pending_work();
/// assert_eq!(work.pending_now, 0);
/// assert_eq!(work.pending_later, 0);
/// runtime.cancel()?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PendingWork {
    /// Messages, source events, and due timers ready to run.
    pub pending_now: usize,
    /// Effects, future timers, active inputs, and requests awaiting replies.
    pub pending_later: usize,
}

/// The number of updates processed and work left after driving a controlled runtime.
///
/// Returned by methods such as [`ControlledRuntime::run_until_idle`] and
/// [`ControlledRuntime::advance`]. See their examples for driving the runtime.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunReport {
    /// The number of component updates processed.
    pub transitions: usize,
    /// Work still immediately runnable when the operation stopped.
    pub pending_now: usize,
    /// Work waiting for input or a later time.
    pub pending_later: usize,
}

/// Time elapsed on a controlled runtime's clock.
///
/// Time advances only when the test asks it to. Trace records use this value;
/// [`Self::as_duration`] returns the elapsed duration. See [`ControlledRuntime::advance`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LogicalTime(Duration);

impl LogicalTime {
    /// Returns the time elapsed since the controlled runtime started.
    pub fn as_duration(self) -> Duration {
        self.0
    }
}

/// Identifies one event within a controlled run's trace.
///
/// Use it to follow [`TraceRecord::cause`] links. IDs are repeatable for identical
/// controlled runs, but are not global identifiers. See [`TraceRecord`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TraceId(u64);

impl TraceId {
    /// Returns the numeric ID within this run.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// One recorded event with its test-clock time and the event that caused it.
///
/// Initialization and test inputs have no `cause`; other events point to their
/// immediate cause by [`TraceId`]. The event payload describes what happened
/// without copying application data.
///
/// ```
/// use samara::TraceRecord;
/// fn caused_by(records: &[TraceRecord], event: &TraceRecord) -> Option<TraceRecord> {
///     let cause = event.cause?;
///     records.iter().find(|record| record.id == cause).cloned()
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceRecord {
    /// This event's ID within the run.
    pub id: TraceId,
    /// The test clock's time when the event occurred.
    pub at: LogicalTime,
    /// The event that caused this one; `None` for initialization and test inputs.
    pub cause: Option<TraceId>,
    /// What happened.
    pub event: TraceEvent,
}

/// The kind of command recorded by [`TraceEvent::CommandEmitted`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceCommandKind {
    /// An effect was requested.
    Effect,
    /// A message was sent to another component.
    Send,
    /// A notification was sent through a port.
    Notify,
    /// A request was sent through a port.
    Request,
    /// A provider replied to a request.
    Reply,
    /// A message was scheduled for later.
    Timer,
}

/// How Samara updated a subscription after a component update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubscriptionAction {
    /// A new input was started.
    Started,
    /// The input stayed active with the same capability and configuration, using
    /// the newest event mapper.
    Retained,
    /// Changed configuration or capability stopped the previous input and started
    /// another.
    Replaced,
    /// Removing the subscription stopped the input.
    Cancelled,
}

/// Whether an effect succeeded, failed, or was cancelled, without its result data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectOutcomeKind {
    /// The effect succeeded.
    Succeeded,
    /// The effect failed.
    Failed,
    /// The supplied effect result was cancellation.
    Cancelled,
}

/// Whether a request received a reply or timed out, without its reply data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestOutcomeKind {
    /// A reply was accepted before any configured deadline.
    Replied,
    /// The request timed out.
    TimedOut,
}

/// An event recorded during controlled execution.
///
/// Records component updates, commands, source changes, results, and errors.
/// [`TraceRecord`] adds a timestamp and causal link. Payloads contain type names
/// and IDs rather than copies of messages or effect arguments.
///
/// ```
/// use samara::{TraceEvent, TraceRecord};
/// fn updates(records: &[TraceRecord]) -> usize {
///     records.iter().filter(|record| matches!(record.event, TraceEvent::Transition { .. })).count()
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceEvent {
    /// A component was initialized.
    Initialization {
        /// The component being initialized.
        component: ComponentId,
    },
    /// A test queued a message for a component.
    ControlledMessageInput {
        /// The receiving component.
        component: ComponentId,
        /// The message's Rust type name.
        message_type: &'static str,
    },
    /// A test supplied a source event.
    ControlledSourceInput {
        /// The component subscribed to the input.
        component: ComponentId,
        /// The subscription name within the component.
        subscription: SubscriptionId,
        /// The underlying input descriptor type, before decoding.
        source_descriptor_type: &'static str,
        /// Whether the input was an item, error, or EOF.
        event: SourceEventKind,
    },
    /// A component processed a message.
    Transition {
        /// The component that handled the message.
        component: ComponentId,
        /// The message's Rust type name.
        message_type: &'static str,
    },
    /// A command was issued at startup, by an update, or by an effect Layer.
    CommandEmitted {
        /// The component that requested the work.
        component: ComponentId,
        /// The kind of command.
        kind: TraceCommandKind,
        /// The descriptor, request, notification, reply, or message type.
        detail_type: &'static str,
        /// The destination component, if addressed directly.
        target_component: Option<ComponentId>,
        /// The destination port, if used.
        target_port: Option<PortId>,
    },
    /// Samara compared a component's subscriptions with its active inputs.
    SubscriptionLifecycle {
        /// The component requesting the subscription.
        component: ComponentId,
        /// The subscription name within the component.
        subscription: SubscriptionId,
        /// The subscribed descriptor type, including decoder wrappers.
        source_descriptor_type: &'static str,
        /// Whether the input was started, kept, replaced, or stopped.
        action: SubscriptionAction,
    },
    /// An input event passed through its decoders to the subscription mapper.
    SourceEventMapped {
        /// The component subscribed to the input.
        component: ComponentId,
        /// The subscription name within the component.
        subscription: SubscriptionId,
        /// The subscribed descriptor type, including decoder wrappers.
        source_descriptor_type: &'static str,
        /// Whether the mapped event was an item, error, or EOF.
        event: SourceEventKind,
    },
    /// An event or message from a replaced or removed input was discarded.
    StaleSourceWorkDropped {
        /// The component that would have received it.
        component: ComponentId,
        /// The subscription name within the component.
        subscription: SubscriptionId,
        /// `true` for an already-mapped message; `false` for an input event.
        mapped_message: bool,
    },
    /// A test supplied an effect result.
    EffectOutcome {
        /// The command that requested this effect.
        effect: TraceId,
        /// The component that requested the effect.
        component: ComponentId,
        /// The effect descriptor type.
        effect_type: &'static str,
        /// The kind of result, without its data.
        outcome: EffectOutcomeKind,
    },
    /// A request received a reply or timed out.
    RequestOutcome {
        /// The requesting component.
        component: ComponentId,
        /// The reply's Rust type name.
        reply_type: &'static str,
        /// The kind of result, without its data.
        outcome: RequestOutcomeKind,
    },
    /// A reply arrived after timeout and was ignored.
    LateReplyDropped {
        /// The provider that sent the late reply.
        component: ComponentId,
        /// The reply's Rust type name.
        reply_type: &'static str,
    },
    /// A timer became due and queued its message.
    TimerFired {
        /// The component receiving the message.
        component: ComponentId,
        /// The message's Rust type name.
        message_type: &'static str,
    },
    /// Execution stopped with a runtime error.
    RuntimeFault {
        /// The component whose work failed.
        component: ComponentId,
        /// The command or subscription event that failed.
        work: TraceId,
        /// The effect or source descriptor type, if applicable.
        descriptor_type: &'static str,
        /// The diagnostic message.
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
                assert_eq!(invocation.reply_to.token.correlation, 41);
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
                assert_eq!(invocation.reply_to.token.correlation, 42);
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

#[cfg(test)]
mod request_authority_tests {
    use super::*;

    struct Replier;

    impl Component for Replier {
        type Model = ();
        type Message = ReplyTo<u64>;

        fn init(&self) -> Init<(), Self::Message> {
            Init::default()
        }

        fn update(&self, _: &mut (), reply_to: Self::Message) -> Command<Self::Message> {
            Command::reply(reply_to, 7)
        }
    }

    fn fixture() -> (Program, ComponentRef<Replier>, ReplyTo<u64>) {
        let mut builder = Program::builder();
        let replier = builder.component(ComponentId::new("replier"), Replier);
        let token = RequestToken::new(0, Arc::new(()));
        token.expire();
        (builder.build().unwrap(), replier, ReplyTo::runtime(token))
    }

    #[test]
    fn controlled_foreign_expired_reply_still_faults() {
        let (program, replier, foreign) = fixture();
        let mut runtime = ControlledRuntime::builder(program).build().unwrap();
        runtime.send(&replier, foreign).unwrap();
        let error = runtime.run_until_idle().unwrap_err();
        assert_eq!(error.component(), Some(replier.id()));
        assert!(error.to_string().contains("another Program"));
        assert!(runtime.cancel().unwrap().is_clean());
    }

    #[tokio::test]
    async fn live_foreign_expired_reply_still_faults() {
        let (program, replier, foreign) = fixture();
        let runtime = LiveRuntime::builder(program).build().unwrap();
        let handle = runtime.handle(&replier).unwrap();
        let task = runtime.spawn();
        handle.send(foreign).await.unwrap();
        let error = task.shutdown(Shutdown::Drain).await.unwrap_err();
        assert_eq!(error.component(), Some(replier.id()));
        assert!(error.to_string().contains("another Program"));
    }
}
