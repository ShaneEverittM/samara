//! Capabilities a Component owns, their creation at registration, and the
//! live Drivers a host supplies for them (ADR-0028, ADR-0029).
use super::*;

mod private {
    use super::live_runtime::LiveBindings;

    pub(crate) trait Driver<C> {
        fn bind(self, capability: &C, bindings: &mut LiveBindings);
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
/// implements it with `#[derive(Capabilities)]`, or by hand:
///
/// ```
/// use samara::prelude::*;
///
/// #[derive(Clone)]
/// pub struct ReaderIo {
///     pub clock: Effect<ReadSystemTime>,
///     pub input: Source<StdinLines>,
/// }
/// impl Capabilities for ReaderIo {
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
pub trait Capabilities: Clone + Send + 'static {
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
/// [`Capabilities::declare`].
pub struct Declaring<'a> {
    builder: &'a mut ProgramBuilder,
    path: Vec<&'static str>,
}

impl Declaring<'_> {
    /// Creates the capabilities for one named field. The name appears in
    /// [`Program::manifest`] and in the error for a missing Driver.
    pub fn field<C: Capabilities>(&mut self, name: &'static str) -> C {
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
fn declare<C: Capabilities>(builder: &mut ProgramBuilder) -> C {
    C::declare(&mut Declaring {
        builder,
        path: Vec::new(),
    })
}

impl<D: EffectDescriptor> Capabilities for Effect<D> {
    fn declare(new: &mut Declaring<'_>) -> Self {
        let name = new.name();
        new.builder.effect::<D>(name)
    }
}
impl<S: SourceDescriptor> Capabilities for Source<S> {
    fn declare(new: &mut Declaring<'_>) -> Self {
        let name = new.name();
        new.builder.source::<S>(name)
    }
}
#[cfg(feature = "fs")]
impl<P: FilePermissions> Capabilities for File<P> {
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
impl Capabilities for () {
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
        impl<$($c: Capabilities),+> Capabilities for ($($c,)+) {
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

/// A live Program awaiting Drivers for its capabilities.
///
/// Supply each capability its Driver with [`Self::supply`], in any order,
/// then [`Self::build`]:
///
/// ```
/// use samara::prelude::*;
/// struct Clock;
/// impl Component for Clock {
///     type Config = (); type Io = Effect<ReadSystemTime>;
///     type Model = (); type Message = ();
///     fn init(_: &(), _: &Self::Io) -> Init<(), ()> { Init::default() }
///     fn update(_: &(), _: &Self::Io, _: &mut (), _: ()) -> Command<()> { Command::none() }
/// }
/// let mut program = Program::builder();
/// let clock = program.component::<Clock>(()).finish()?;
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
/// #     type Config = (); type Io = Effect<ReadSystemTime>;
/// #     type Model = (); type Message = ();
/// #     fn init(_: &(), _: &Self::Io) -> Init<(), ()> { Init::default() }
/// #     fn update(_: &(), _: &Self::Io, _: &mut (), _: ()) -> Command<()> { Command::none() }
/// # }
/// let mut program = Program::builder();
/// program.component::<Clock>(()).name("clock").finish()?;
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

    /// Validates that every capability has exactly one Driver and builds the
    /// runtime.
    ///
    /// A missing, duplicate, or foreign binding returns an error naming the
    /// owning Component and the descriptor type, as do foreign capabilities
    /// in startup commands and subscriptions. Call [`Runtime::spawn`] to
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

/// A Component registration.
///
/// `finish()` creates the Component's own capabilities and returns a handle
/// carrying them for the host:
///
/// ```
/// use samara::prelude::*;
/// struct Clock;
/// impl Component for Clock {
///     type Config = (); type Io = (Effect<ReadSystemTime>,);
///     type Model = (); type Message = ();
///     fn init(_: &(), _: &Self::Io) -> Init<(), ()> { Init::default() }
///     fn update(_: &(), _: &Self::Io, _: &mut (), _: ()) -> Command<()> { Command::none() }
/// }
/// let mut program = Program::builder();
/// let clock = program.component::<Clock>(()).finish()?;
/// let runtime = Runtime::builder(program.build()?)
///     .supply(&clock.io.0, SystemClockDriver)
///     .build()?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[must_use = "finish component registration with .finish()"]
pub struct ComponentRegistration<'a, C: Component> {
    program: &'a mut ProgramBuilder,
    name: Option<Arc<str>>,
    config: C::Config,
}
impl<'a, C: Component> ComponentRegistration<'a, C> {
    pub(crate) fn new(program: &'a mut ProgramBuilder, config: C::Config) -> Self {
        Self {
            program,
            name: None,
            config,
        }
    }

    /// Sets a diagnostic label. Names need not be unique and never affect routing
    /// or execution order. Without this, diagnostics use the Component type name.
    pub fn name(mut self, name: impl Into<Arc<str>>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Creates the Component's capabilities, initializes it, and registers it.
    pub fn finish(self) -> Result<InstalledComponent<C>, ProgramBuildError> {
        let mark = self.program.requirement_mark();
        let io = declare::<C::Io>(self.program);
        let reference = self
            .program
            .register::<C>(self.name, self.config, io.clone())?;
        self.program.claim(mark, reference.id());
        Ok(InstalledComponent { io, reference })
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
    fn install(
        program: &mut ProgramBuilder,
        config: Self::Config,
    ) -> Result<(Self::Exports, Self::Io), ProgramBuildError>;
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
    ///     fn install(_: &mut ProgramBuilder, _: ()) -> Result<(Doors, ()), ProgramBuildError> {
    ///         unimplemented!()
    ///     }
    /// }
    /// fn which_door(installed: &InstalledAssembly<Telemetry>) {
    ///     let _ = installed.connector();
    /// }
    /// ```
    pub fn connector(&self) -> &Connector<M> {
        &self.exports
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
/// A staged Assembly installation.
#[must_use = "finish Assembly installation with .finish()"]
pub struct AssemblyRegistration<'a, A: Assembly> {
    program: &'a mut ProgramBuilder,
    config: A::Config,
}
impl ProgramBuilder {
    /// Starts installing an Assembly with its settings and connections.
    pub fn install<A: Assembly>(&mut self, config: A::Config) -> AssemblyRegistration<'_, A> {
        AssemblyRegistration {
            program: self,
            config,
        }
    }
}
impl<A: Assembly> AssemblyRegistration<'_, A> {
    /// Installs the Assembly. Discard the ProgramBuilder on failure.
    pub fn finish(self) -> Result<InstalledAssembly<A>, ProgramBuildError> {
        let (exports, io) = A::install(self.program, self.config)?;
        Ok(InstalledAssembly { io, exports })
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
