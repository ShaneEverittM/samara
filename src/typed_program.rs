//! Capabilities a Component owns, their creation at registration, and the
//! live Drivers a host supplies for them (ADR-0028, ADR-0029).
use super::*;

mod private {
    use super::live_runtime::LiveBindings;

    pub(crate) trait Driver<C> {
        fn bind(self, capability: &C, bindings: &mut LiveBindings);
    }

    /// A capability whose descriptor has one obvious live Driver.
    pub(crate) trait DefaultDriver {
        fn bind_default(&self, bindings: &mut LiveBindings);
    }

    /// An effect descriptor's obvious live Driver.
    pub(crate) trait EffectDefault: crate::EffectDescriptor + Sized {
        fn driver() -> impl crate::EffectDriver<Self>;
    }

    /// A terminal source descriptor's obvious live Driver, bound to any
    /// source layered over it.
    pub(crate) trait SourceDefault: crate::SourceDescriptor {
        fn bind<S>(capability: &crate::Source<S>, bindings: &mut LiveBindings)
        where
            S: crate::SourceDescriptor<Terminal = Self>;
    }
}

/// The capabilities a Component owns: its whole access to the world.
///
/// A Component names one as [`Component::Io`]. Registration creates a fresh
/// value for that Component, hands it to every `init`, `update`,
/// `subscriptions`, and `shutdown` call, and exposes the same value on the
/// handle registration returns so the host can supply Drivers.
///
/// The library implements it for [`Effect`], [`Source`], `File` (with the
/// `fs` feature), `()`, and tuples of those. A struct with named fields
/// implements it with `#[derive(Io)]`, or by hand:
///
/// ```
/// use samara::prelude::*;
///
/// #[derive(Clone)]
/// pub struct ReaderIo {
///     pub clock: Effect<ReadSystemTime>,
///     pub input: Source<StdinLines>,
/// }
/// impl Io for ReaderIo {
///     fn declare(new: &mut Declaring<'_>) -> Self {
///         Self {
///             clock: new.field("clock"),
///             input: new.field("input"),
///         }
///     }
/// }
/// ```
///
/// A capability cannot be made any other way:
///
/// ```compile_fail
/// use samara::prelude::*;
/// let clock: Effect<ReadSystemTime> = Effect::new();
/// ```
pub trait Io: Clone + Send + 'static {
    /// Creates one fresh capability per field.
    fn declare(new: &mut Declaring<'_>) -> Self;

    /// Capabilities that belong to no Program, for calling a Component's
    /// `init`, `update`, or `subscriptions` directly in a unit test and
    /// inspecting what they return.
    ///
    /// They cannot be supplied with Drivers, and a runtime faults if a
    /// Component uses one.
    ///
    /// ```
    /// use samara::prelude::*;
    /// let clock = <Effect<ReadSystemTime>>::detached();
    /// let command: Command<()> = Effect::perform(&clock, ReadSystemTime).discard_outcome();
    /// assert!(command.effect_intent::<ReadSystemTime>().is_some());
    /// ```
    fn detached() -> Self {
        declare(&mut Program::builder())
    }
}

/// Creates capabilities for the Component being registered. Passed to
/// [`Io::declare`].
pub struct Declaring<'a> {
    builder: &'a mut ProgramBuilder,
    path: Vec<&'static str>,
}

impl Declaring<'_> {
    /// Creates the capabilities for one named field. The name appears in
    /// [`Program::manifest`] and in the error for a missing Driver.
    pub fn field<C: Io>(&mut self, name: &'static str) -> C {
        self.path.push(name);
        let capabilities = C::declare(self);
        self.path.pop();
        capabilities
    }

    fn name(&self) -> Option<Arc<str>> {
        (!self.path.is_empty()).then(|| self.path.join(".").into())
    }
}

/// Creates the capabilities one registration owns.
fn declare<C: Io>(builder: &mut ProgramBuilder) -> C {
    C::declare(&mut Declaring {
        builder,
        path: Vec::new(),
    })
}

impl<D: EffectDescriptor> Io for Effect<D> {
    fn declare(new: &mut Declaring<'_>) -> Self {
        let name = new.name();
        new.builder.effect::<D>(name)
    }
}
impl<S: SourceDescriptor> Io for Source<S> {
    fn declare(new: &mut Declaring<'_>) -> Self {
        let name = new.name();
        new.builder.source::<S>(name)
    }
}
#[cfg(feature = "fs")]
impl<P: FilePermissions> Io for File<P> {
    fn declare(new: &mut Declaring<'_>) -> Self {
        let name = new.name();
        let token = new.builder.next_capability();
        if P::READ {
            new.builder.exact_effect::<FileRead>(&token, name.clone());
        }
        if P::APPEND {
            new.builder.exact_effect::<FileAppend>(&token, name.clone());
        }
        if P::WRITE {
            new.builder.exact_effect::<FileReplace>(&token, name);
        }
        File::new(token)
    }
}
impl Io for () {
    fn declare(_: &mut Declaring<'_>) {}
}

/// Live implementations for a capability or a tuple of capabilities.
///
/// An [`EffectDriver`] supplies an [`Effect`]. A source Driver handles the
/// terminal descriptor beneath its declared Layers: wrap a [`SourceDriver`]
/// in [`DrivenSource`], or use [`MpscReceiver`] for a channel and
/// `StdinDriver` for stdin. With the `fs` feature, one `FileDriver` supplies
/// every operation of a `File`. A tuple of Drivers supplies a tuple of
/// capabilities position by position.
///
/// Sealed. Implement [`EffectDriver`] or [`SourceDriver`] for your own
/// Drivers and pass them to [`RuntimeSetup::supply`].
#[allow(private_bounds)] // Sealed marker; registry access stays inside the crate.
pub trait Drivers<C>: private::Driver<C> {}
impl<C, T: private::Driver<C>> Drivers<C> for T {}

impl<D: EffectDescriptor, T: EffectDriver<D>> private::Driver<Effect<D>> for T {
    fn bind(self, capability: &Effect<D>, bindings: &mut live_runtime::LiveBindings) {
        bindings.bind_effect::<D, T>(&capability.token, self);
    }
}
#[cfg(feature = "fs")]
impl<P: FilePermissions> private::Driver<File<P>> for FileDriver {
    fn bind(self, capability: &File<P>, bindings: &mut live_runtime::LiveBindings) {
        if P::READ {
            bindings.bind_effect::<FileRead, _>(&capability.token, self.clone());
        }
        if P::APPEND {
            bindings.bind_effect::<FileAppend, _>(&capability.token, self.clone());
        }
        if P::WRITE {
            bindings.bind_effect::<FileReplace, _>(&capability.token, self);
        }
    }
}
/// A terminal source Driver supplied for one [`Source`] capability.
pub struct DrivenSource<T>(T);
impl<T> DrivenSource<T> {
    /// Wraps a terminal Driver; Samara still runs all declared source Layers.
    pub fn new(driver: T) -> Self {
        Self(driver)
    }
}
impl<S: SourceDescriptor, T: SourceDriver<S::Terminal>> private::Driver<Source<S>>
    for DrivenSource<T>
{
    fn bind(self, capability: &Source<S>, bindings: &mut live_runtime::LiveBindings) {
        bindings.bind_source::<S::Terminal, T>(&capability.token, self.0);
    }
}
impl private::Driver<()> for () {
    fn bind(self, _: &(), _: &mut live_runtime::LiveBindings) {}
}
macro_rules! tuples {
    ($(($($c:ident $d:ident $i:tt),+))+) => {$(
        impl<$($c: Io),+> Io for ($($c,)+) {
            fn declare(new: &mut Declaring<'_>) -> Self {
                ($($c::declare(new),)+)
            }
        }
        impl<$($c, $d: Drivers<$c>),+> private::Driver<($($c,)+)> for ($($d,)+) {
            fn bind(self, capabilities: &($($c,)+), bindings: &mut live_runtime::LiveBindings) {
                $(private::Driver::bind(self.$i, &capabilities.$i, bindings);)+
            }
        }
    )+};
}
tuples! {
    (C1 D1 0)
    (C1 D1 0, C2 D2 1)
    (C1 D1 0, C2 D2 1, C3 D3 2)
    (C1 D1 0, C2 D2 1, C3 D3 2, C4 D4 3)
    (C1 D1 0, C2 D2 1, C3 D3 2, C4 D4 3, C5 D5 4)
    (C1 D1 0, C2 D2 1, C3 D3 2, C4 D4 3, C5 D5 4, C6 D6 5)
    (C1 D1 0, C2 D2 1, C3 D3 2, C4 D4 3, C5 D5 4, C6 D6 5, C7 D7 6)
    (C1 D1 0, C2 D2 1, C3 D3 2, C4 D4 3, C5 D5 4, C6 D6 5, C7 D7 6, C8 D8 7)
}

/// A one-shot channel receiver supplied for one stream [`Source`] capability.
pub struct MpscReceiver<T>(tokio::sync::mpsc::Receiver<T>);
impl<T> MpscReceiver<T> {
    /// Transfers a receiver into the capability it is supplied for.
    pub fn new(receiver: tokio::sync::mpsc::Receiver<T>) -> Self {
        Self(receiver)
    }
}
impl<S: StreamSourceDescriptor> private::Driver<Source<S>> for MpscReceiver<S::StreamItem> {
    fn bind(self, capability: &Source<S>, bindings: &mut live_runtime::LiveBindings) {
        bindings.bind_mpsc(capability, self.0);
    }
}

/// Binds the capability it is supplied for to the process stdin reader.
///
/// Supply this value for a `Source<StdinLines>` capability, or a composed
/// source with that terminal descriptor, through [`RuntimeSetup::supply`].
/// A second process-input binding fails runtime build.
/// Only one active subscription may read stdin across all runtimes. A competing
/// reader faults the runtime; application code must not read stdin concurrently.
/// Removing the subscription or shutting down interrupts and joins the reader.
/// A later subscription resumes at stdin's current position.
#[cfg(unix)]
pub struct StdinDriver;
#[cfg(unix)]
impl<S: SourceDescriptor<Terminal = StdinLines>> private::Driver<Source<S>> for StdinDriver {
    fn bind(self, capability: &Source<S>, bindings: &mut live_runtime::LiveBindings) {
        bindings.bind_stdin_capability(capability);
    }
}

/// A capability that has one obvious live Driver, for
/// [`RuntimeSetup::supply_default`].
///
/// Implemented for `Effect<PrintStdout>`, `Effect<PrintStderr>`,
/// `Effect<ReadSystemTime>`, `Effect<HttpRequest>`, any [`Source`] whose
/// terminal descriptor is [`StdinLines`] or [`TcpBytes`], `()`, references
/// to those, and tuples of any of them. A capability whose Driver needs a setting, such as a `File`'s
/// path, has no default. Sealed.
#[allow(private_bounds)] // Sealed marker; registry access stays inside the crate.
pub trait HasDefaultDriver: private::DefaultDriver {}
impl<T: private::DefaultDriver> HasDefaultDriver for T {}

impl<D: private::EffectDefault> private::DefaultDriver for Effect<D> {
    fn bind_default(&self, bindings: &mut live_runtime::LiveBindings) {
        private::Driver::bind(D::driver(), self, bindings);
    }
}
impl<S> private::DefaultDriver for Source<S>
where
    S: SourceDescriptor,
    S::Terminal: private::SourceDefault,
{
    fn bind_default(&self, bindings: &mut live_runtime::LiveBindings) {
        <S::Terminal as private::SourceDefault>::bind(self, bindings);
    }
}
impl private::DefaultDriver for () {
    fn bind_default(&self, _: &mut live_runtime::LiveBindings) {}
}
impl<T: private::DefaultDriver + ?Sized> private::DefaultDriver for &T {
    fn bind_default(&self, bindings: &mut live_runtime::LiveBindings) {
        (**self).bind_default(bindings);
    }
}
macro_rules! default_tuples {
    ($(($($c:ident),+))+) => {$(
        impl<$($c: private::DefaultDriver),+> private::DefaultDriver for ($($c,)+) {
            fn bind_default(&self, bindings: &mut live_runtime::LiveBindings) {
                #[allow(non_snake_case)]
                let ($($c,)+) = self;
                $($c.bind_default(bindings);)+
            }
        }
    )+};
}
default_tuples! {
    (A)
    (A, B)
    (A, B, C)
    (A, B, C, D)
    (A, B, C, D, E)
    (A, B, C, D, E, F)
    (A, B, C, D, E, F, G)
    (A, B, C, D, E, F, G, H)
}
impl private::EffectDefault for PrintStdout {
    fn driver() -> impl EffectDriver<Self> {
        StdoutDriver::new()
    }
}
impl private::EffectDefault for PrintStderr {
    fn driver() -> impl EffectDriver<Self> {
        StderrDriver::new()
    }
}
impl private::EffectDefault for ReadSystemTime {
    fn driver() -> impl EffectDriver<Self> {
        SystemClockDriver
    }
}
impl private::EffectDefault for HttpRequest {
    fn driver() -> impl EffectDriver<Self> {
        HttpDriver::new()
    }
}
#[cfg(unix)]
impl private::SourceDefault for StdinLines {
    fn bind<S>(capability: &Source<S>, bindings: &mut live_runtime::LiveBindings)
    where
        S: SourceDescriptor<Terminal = Self>,
    {
        private::Driver::bind(StdinDriver, capability, bindings);
    }
}
impl private::SourceDefault for TcpBytes {
    fn bind<S>(capability: &Source<S>, bindings: &mut live_runtime::LiveBindings)
    where
        S: SourceDescriptor<Terminal = Self>,
    {
        private::Driver::bind(DrivenSource::new(TcpDriver), capability, bindings);
    }
}

/// A live Program awaiting Drivers for its capabilities.
///
/// Supply each capability its Driver with [`Self::supply`], in any order,
/// then [`Self::build`]:
///
/// ```
/// use samara::prelude::*;
/// struct Clock;
/// impl Component for Clock {
///     type Connections = (); type Configuration = (); type Io = Effect<ReadSystemTime>;
///     type Model = (); type Message = ();
///     fn init(_: &Context<Self>) -> Init<(), ()> { Init::default() }
///     fn update(_: &Context<Self>, _: &mut (), _: ()) -> Command<()> { Command::none() }
/// }
/// let mut program = Program::builder();
/// let clock = program.add_component::<Clock>(());
/// let runtime = Runtime::builder(program.build()?)
///     .supply(&clock.io, SystemClockDriver)
///     .build()?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// A Driver of the wrong type for its capability does not compile:
///
/// ```compile_fail
/// use samara::prelude::*;
/// fn wrong(setup: samara::RuntimeSetup, clock: &Effect<ReadSystemTime>) {
///     setup.supply(clock, samara::HttpDriver::new());
/// }
/// ```
///
/// A capability left without a Driver, or given two, fails `build`, even for
/// a boundary first used after a later Message:
///
/// ```
/// use samara::prelude::*;
/// # struct Clock;
/// # impl Component for Clock {
/// #     type Connections = (); type Configuration = (); type Io = Effect<ReadSystemTime>;
/// #     type Model = (); type Message = ();
/// #     fn init(_: &Context<Self>) -> Init<(), ()> { Init::default() }
/// #     fn update(_: &Context<Self>, _: &mut (), _: ()) -> Command<()> { Command::none() }
/// # }
/// let mut program = Program::builder();
/// program.configure_component::<Clock>().name("clock").add(());
/// let error = Runtime::builder(program.build()?).build().err().unwrap();
/// assert!(error.to_string().contains("ReadSystemTime"));
/// assert!(error.to_string().contains("clock"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// Bindings can only be added this way:
///
/// ```compile_fail
/// use samara::prelude::*;
/// let program = Program::builder();
/// Runtime::builder(program.build().unwrap()).bind_effect::<ReadSystemTime, _>(SystemClockDriver);
/// ```
#[must_use = "supply Drivers with supply(&capability, driver) and then build()"]
pub struct RuntimeSetup {
    pub(crate) builder: RuntimeBuilder,
}
impl RuntimeSetup {
    /// Supplies the live Drivers for one capability or a tuple of them.
    ///
    /// The Driver goes to exactly the capability named here, which is the one
    /// its Component uses. The Driver types are checked against the
    /// capability's shape; whether every capability was supplied exactly once
    /// is checked by [`Self::build`].
    pub fn supply<C>(mut self, capability: &C, drivers: impl Drivers<C>) -> Self {
        private::Driver::bind(drivers, capability, &mut self.builder.bindings);
        self
    }

    /// Supplies a capability its standard live Driver: stdin, stdout, stderr,
    /// the system clock, HTTP, or TCP, by the capability's type.
    ///
    /// It is `supply` with the obvious Driver filled in, and counts as that
    /// capability's one Driver. Pass a reference to one capability, or a tuple
    /// of references to supply several at once:
    /// `.supply_default((&journal.io.clock, &monitor.io.input))`.
    ///
    /// ```
    /// use samara::prelude::*;
    /// # struct Clock;
    /// # impl Component for Clock {
    /// #     type Connections = (); type Configuration = (); type Io = Effect<ReadSystemTime>;
    /// #     type Model = (); type Message = ();
    /// #     fn init(_: &Context<Self>) -> Init<(), ()> { Init::default() }
    /// #     fn update(_: &Context<Self>, _: &mut (), _: ()) -> Command<()> { Command::none() }
    /// # }
    /// let mut program = Program::builder();
    /// let clock = program.add_component::<Clock>(());
    /// let runtime = Runtime::builder(program.build()?)
    ///     .supply_default(&clock.io)
    ///     .build()?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// A capability whose Driver needs a setting has no default:
    ///
    /// ```compile_fail
    /// use samara::prelude::*;
    /// fn no_path(setup: samara::RuntimeSetup, journal: &File<ReadAppend>) {
    ///     setup.supply_default(journal);
    /// }
    /// ```
    pub fn supply_default(mut self, capabilities: impl HasDefaultDriver) -> Self {
        capabilities.bind_default(&mut self.builder.bindings);
        self
    }

    /// Validates that every capability has exactly one Driver and builds the
    /// runtime.
    ///
    /// A missing binding returns an error naming the owning Component, the
    /// field, and the descriptor type. A duplicate or foreign binding, and a
    /// foreign capability in a startup command or subscription, return an
    /// error naming the descriptor type. Call [`Runtime::spawn`] to
    /// begin work.
    pub fn build(self) -> Result<Runtime, RuntimeError> {
        self.builder.build()
    }
}

/// A controlled Program. Every capability it declared is controlled by the
/// test; no Driver runs.
#[must_use = "build the controlled runtime with build()"]
pub struct ControlledRuntimeSetup {
    pub(crate) builder: ControlledRuntimeBuilder,
}
impl ControlledRuntimeSetup {
    /// Controls every declared effect and source, preserving exact resource identities.
    pub fn control_all(self) -> ControlledRuntimeBuilder {
        self.builder.control_declared()
    }

    /// Controls everything and builds. The same as `control_all().build()`.
    pub fn build(self) -> Result<ControlledRuntime, RuntimeError> {
        self.control_all().build()
    }
}

/// The other Components one Component can send to: its
/// [`Component::Connections`].
///
/// Implemented for `()`, [`Connector`], [`Port`], [`ComponentRef`],
/// `Option` and `Vec` of those, tuples up to eight, and structs with named
/// fields through `#[derive(Connections)]`. Samara walks the value at
/// registration so that [`Program::manifest`] can list who sends to whom
/// and [`ProgramBuilder::build`] can reject an address from another
/// builder.
///
/// ```
/// use samara::prelude::*;
/// # enum JournalMessage {}
/// # enum AuditMessage {}
/// #[derive(Connections)]
/// struct Wiring {
///     journal: Connector<JournalMessage>,
///     audit: Port<AuditMessage>,
/// }
/// ```
///
/// Written by hand, an implementation names each field:
///
/// ```
/// use samara::prelude::*;
/// # enum JournalMessage {}
/// struct Wiring { journal: Connector<JournalMessage> }
/// impl Connections for Wiring {
///     fn list(&self, into: &mut Listing<'_>) {
///         into.field("journal", &self.journal);
///     }
/// }
/// ```
pub trait Connections: Send + 'static {
    /// Lists each connection this value holds.
    fn list(&self, into: &mut Listing<'_>);
}

/// Collects the connections of the Component being registered. Passed to
/// [`Connections::list`].
pub struct Listing<'a> {
    listed: &'a mut Vec<ListedConnection>,
    path: Vec<&'static str>,
}

impl<'a> Listing<'a> {
    pub(crate) fn new(listed: &'a mut Vec<ListedConnection>) -> Self {
        Self {
            listed,
            path: Vec::new(),
        }
    }

    /// Lists the connections held in one named field. The name appears in
    /// [`Program::manifest`] and in the error for a foreign connection.
    pub fn field<C: Connections + ?Sized>(&mut self, name: &'static str, connections: &C) {
        self.path.push(name);
        connections.list(self);
        self.path.pop();
    }

    fn push(&mut self, program: &Arc<()>, message: &'static str, target: ListedTarget) {
        self.listed.push(ListedConnection {
            name: (!self.path.is_empty()).then(|| self.path.join(".")),
            program: program.clone(),
            message,
            target,
        });
    }
}

/// One address found in a Component's connections, before `build` checks
/// it and resolves a Port to its provider.
pub(crate) struct ListedConnection {
    pub(crate) name: Option<String>,
    pub(crate) program: Arc<()>,
    pub(crate) message: &'static str,
    pub(crate) target: ListedTarget,
}

pub(crate) enum ListedTarget {
    Component(ComponentId),
    Port { protocol: TypeId, id: PortId },
}

impl Connections for () {
    fn list(&self, _: &mut Listing<'_>) {}
}
impl<M: Send + 'static> Connections for Connector<M> {
    fn list(&self, into: &mut Listing<'_>) {
        into.push(
            &self.program,
            std::any::type_name::<M>(),
            ListedTarget::Component(self.id.clone()),
        );
    }
}
impl<C: Component> Connections for ComponentRef<C> {
    fn list(&self, into: &mut Listing<'_>) {
        into.push(
            &self.program,
            std::any::type_name::<C::Message>(),
            ListedTarget::Component(self.id.clone()),
        );
    }
}
impl<P: Send + 'static> Connections for Port<P> {
    fn list(&self, into: &mut Listing<'_>) {
        into.push(
            &self.program,
            std::any::type_name::<P>(),
            ListedTarget::Port {
                protocol: TypeId::of::<P>(),
                id: self.id.clone(),
            },
        );
    }
}
impl<T: Connections> Connections for Option<T> {
    fn list(&self, into: &mut Listing<'_>) {
        if let Some(connections) = self {
            connections.list(into);
        }
    }
}
impl<T: Connections> Connections for Vec<T> {
    fn list(&self, into: &mut Listing<'_>) {
        for connections in self {
            connections.list(into);
        }
    }
}
macro_rules! connection_tuples {
    ($(($($c:ident),+))+) => {$(
        impl<$($c: Connections),+> Connections for ($($c,)+) {
            fn list(&self, into: &mut Listing<'_>) {
                #[allow(non_snake_case)]
                let ($($c,)+) = self;
                $($c.list(into);)+
            }
        }
    )+};
}
connection_tuples! {
    (A)
    (A, B)
    (A, B, C)
    (A, B, C, D)
    (A, B, C, D, E)
    (A, B, C, D, E, F)
    (A, B, C, D, E, F, G)
    (A, B, C, D, E, F, G, H)
}

/// Marks a [`ComponentBuilder`] whose configuration has not been set, so
/// registration uses the `Default` value.
pub struct DefaultConfiguration;
/// Marks a [`ComponentBuilder`] whose configuration was set with
/// [`ComponentBuilder::config`].
pub struct GivenConfiguration<T>(T);

mod configuration_private {
    #[diagnostic::on_unimplemented(
        message = "`{T}` has no default value to register this Component with",
        note = "set one with `.configure_component::<C>().config(..).add(..)`, or implement `Default` for `{T}`"
    )]
    pub trait Slot<T> {
        fn take(self) -> T;
    }
}
use configuration_private::Slot;
impl<T: Default> Slot<T> for DefaultConfiguration {
    fn take(self) -> T {
        T::default()
    }
}
impl<T> Slot<T> for GivenConfiguration<T> {
    fn take(self) -> T {
        self.0
    }
}

/// Registers a Component with options. Made by
/// [`ProgramBuilder::configure_component`]; use [`ProgramBuilder::add_component`]
/// when there is nothing to set.
///
/// ```
/// use samara::prelude::*;
/// struct Clock;
/// impl Component for Clock {
///     type Io = (Effect<ReadSystemTime>,);
///     type Connections = (); type Configuration = u64;
///     type Model = (); type Message = ();
///     fn init(_: &Context<Self>) -> Init<(), ()> { Init::default() }
///     fn update(_: &Context<Self>, _: &mut (), _: ()) -> Command<()> { Command::none() }
/// }
/// let mut program = Program::builder();
/// let clock = program
///     .configure_component::<Clock>()
///     .name("wall clock")
///     .config(60)
///     .add(());
/// assert_eq!(clock.id().name(), "wall clock");
/// let runtime = Runtime::builder(program.build()?)
///     .supply(&clock.io.0, SystemClockDriver)
///     .build()?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// A Component whose configuration has no `Default` must be given one:
///
/// ```compile_fail
/// use samara::prelude::*;
/// struct Settings { limit: u64 }
/// struct Limiter;
/// impl Component for Limiter {
///     type Io = ();
///     type Connections = (); type Configuration = Settings;
///     type Model = (); type Message = ();
///     fn init(_: &Context<Self>) -> Init<(), ()> { Init::default() }
///     fn update(_: &Context<Self>, _: &mut (), _: ()) -> Command<()> { Command::none() }
/// }
/// let mut program = Program::builder();
/// program.add_component::<Limiter>(());
/// ```
#[must_use = "register the Component with .add(connections)"]
pub struct ComponentBuilder<'a, C: Component, Configuration = DefaultConfiguration> {
    program: &'a mut ProgramBuilder,
    name: Option<Arc<str>>,
    configuration: Configuration,
    kind: PhantomData<fn() -> C>,
}
impl<'a, C: Component> ComponentBuilder<'a, C> {
    pub(crate) fn new(program: &'a mut ProgramBuilder) -> Self {
        Self {
            program,
            name: None,
            configuration: DefaultConfiguration,
            kind: PhantomData,
        }
    }

    /// Sets the Component's configuration. Without this, registration uses
    /// its `Default` value.
    pub fn config(
        self,
        configuration: C::Configuration,
    ) -> ComponentBuilder<'a, C, GivenConfiguration<C::Configuration>> {
        ComponentBuilder {
            program: self.program,
            name: self.name,
            configuration: GivenConfiguration(configuration),
            kind: PhantomData,
        }
    }
}
impl<C: Component, Configuration> ComponentBuilder<'_, C, Configuration> {
    /// Sets a diagnostic label. Names need not be unique and never affect routing
    /// or execution order. Without this, diagnostics use the Component type name.
    pub fn name(mut self, name: impl Into<Arc<str>>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Creates the Component's capabilities, initializes it, and registers it
    /// with these connections.
    // The name is the builder's verb (ADR-0032), not arithmetic.
    #[allow(private_bounds, clippy::should_implement_trait)]
    pub fn add(self, connections: C::Connections) -> InstalledComponent<C>
    where
        Configuration: Slot<C::Configuration>,
    {
        let mark = self.program.requirement_mark();
        let io = declare::<C::Io>(self.program);
        let context = Context {
            io: io.clone(),
            to: connections,
            config: self.configuration.take(),
        };
        let reference = self.program.register::<C>(self.name, context);
        self.program.claim(mark, reference.id());
        InstalledComponent { io, reference }
    }
}

/// A registered Component together with the capabilities created for it.
///
/// The host reads [`Self::io`] to supply Drivers. Everything a
/// [`ComponentRef`] offers is available through this handle, and it is an
/// address in its own right. Hand other Components the `ComponentRef` or a
/// Connector, never the capabilities: using a capability you do not own is a
/// runtime fault.
pub struct InstalledComponent<C: Component> {
    /// The capabilities created for this Component, for
    /// [`RuntimeSetup::supply`].
    pub io: C::Io,
    reference: ComponentRef<C>,
}
impl<C: Component> InstalledComponent<C> {
    /// The Component's address.
    pub fn reference(&self) -> &ComponentRef<C> {
        &self.reference
    }
}
impl<C: Component> std::ops::Deref for InstalledComponent<C> {
    type Target = ComponentRef<C>;
    fn deref(&self) -> &ComponentRef<C> {
        &self.reference
    }
}
impl<C: Component> From<InstalledComponent<C>> for ComponentRef<C> {
    fn from(installed: InstalledComponent<C>) -> Self {
        installed.reference
    }
}
impl<C: Component> Addressable for InstalledComponent<C> {
    type Message = C::Message;
    fn to_address(&self) -> Address<C::Message> {
        self.reference.to_address()
    }
}
impl<C: Component> fmt::Debug for InstalledComponent<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InstalledComponent")
            .field("component", &std::any::type_name::<C>())
            .finish_non_exhaustive()
    }
}

/// A reusable graph fragment: a function that registers Components and says
/// what the caller gets back.
pub trait Assembly {
    /// Settings and connections supplied by the caller.
    type Config;
    /// What the caller receives: a [`Connector`] for an Assembly with one
    /// door, a struct naming several, or `()` when nobody sends to it.
    type Exports;
    /// The capabilities the host must supply Drivers for: those of the
    /// Components this Assembly registers. Usually one member's
    /// [`Component::Io`], or a struct of several.
    type Io;
    /// Registers the Assembly's Components and returns its exports and its
    /// members' capabilities.
    fn install(program: &mut ProgramBuilder, config: Self::Config) -> (Self::Exports, Self::Io);
}
/// An installed Assembly's exports and capabilities.
pub struct InstalledAssembly<A: Assembly> {
    /// The capabilities of the Assembly's Components, for
    /// [`RuntimeSetup::supply`].
    pub io: A::Io,
    exports: A::Exports,
}
impl<A: Assembly> InstalledAssembly<A> {
    /// What the Assembly exported.
    pub fn exports(&self) -> &A::Exports {
        &self.exports
    }
}
impl<A, M> InstalledAssembly<A>
where
    A: Assembly<Exports = Connector<M>>,
{
    /// The Assembly's address, when its export is a single [`Connector`].
    ///
    /// An Assembly that exports several doors names them as fields; reach
    /// those through [`Self::exports`].
    ///
    /// ```compile_fail
    /// use samara::prelude::*;
    ///
    /// struct Doors {
    ///     samples: Connector<u8>,
    ///     queries: Connector<u16>,
    /// }
    /// struct Telemetry;
    /// impl Assembly for Telemetry {
    ///     type Config = ();
    ///     type Exports = Doors;
    ///     type Io = ();
    ///     fn install(_: &mut ProgramBuilder, _: ()) -> (Doors, ()) {
    ///         unimplemented!()
    ///     }
    /// }
    /// fn which_door(installed: &InstalledAssembly<Telemetry>) {
    ///     let _ = installed.connector();
    /// }
    /// ```
    pub fn connector(&self) -> Connector<M> {
        self.exports.clone()
    }
}
impl<A, M> Addressable for InstalledAssembly<A>
where
    A: Assembly<Exports = Connector<M>>,
    M: Send + 'static,
{
    type Message = M;
    fn to_address(&self) -> Address<M> {
        self.exports.to_address()
    }
}
impl<A: Assembly> fmt::Debug for InstalledAssembly<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InstalledAssembly")
            .field("assembly", &std::any::type_name::<A>())
            .finish_non_exhaustive()
    }
}
impl ProgramBuilder {
    /// Installs an Assembly with its settings and connections.
    pub fn add_assembly<A: Assembly>(&mut self, config: A::Config) -> InstalledAssembly<A> {
        let (exports, io) = A::install(self, config);
        InstalledAssembly { io, exports }
    }
}
/// Optional default live implementations for a library Assembly.
pub trait VendorsDrivers: Assembly {
    /// Live implementation settings, independent of [`Assembly::Config`].
    /// Use `()` when the vendored Drivers have no settings.
    type DriverConfig;
    /// Supplies this Assembly's capabilities with its default live Drivers.
    /// Controlled execution does not need these settings or implementations.
    fn supply(setup: RuntimeSetup, io: &Self::Io, config: Self::DriverConfig) -> RuntimeSetup;
}
