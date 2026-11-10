//! Typed declaration and binding of a closed Program's world boundaries.
use super::*;

mod private {
    use super::{Capabilities, Requirements, live_runtime::LiveBindings};

    pub trait Requirement {}
    pub trait CapabilityValue {}
    pub(crate) trait Driver<R: Requirements> {
        fn bind(self, capabilities: &Capabilities<R>, bindings: &mut LiveBindings);
    }
    pub struct Key;
}

/// The capability values corresponding to a requirement shape.
pub type Capabilities<R> = <R as Requirements>::Capabilities;

/// A finite requirement shape: `Effect<D>`, `Source<S>`, `()`, or nested tuples.
/// The `fs` feature also supplies `File<P>` for one permission-typed file resource.
///
/// Sealed so the typed shape and its issued capabilities cannot disagree.
///
/// Capabilities cannot be added after choosing the Program's type:
///
/// ```compile_fail
/// use samara::prelude::*;
/// let (mut program, ()) = Program::builder::<()>();
/// program.effect::<ReadSystemTime>();
/// ```
///
/// ```compile_fail
/// use samara::prelude::*;
/// let (mut program, ()) = Program::builder::<()>();
/// program.source::<StdinLines>();
/// ```
///
/// ```compile_fail
/// struct Unchecked;
/// impl samara::Requirements for Unchecked { type Capabilities = (); }
/// ```
#[allow(private_interfaces)]
pub trait Requirements: private::Requirement + 'static {
    /// Immutable, clonable capability values passed to Components and Assemblies.
    type Capabilities: Clone + Send + 'static;
    #[doc(hidden)]
    fn declare(builder: &mut ProgramBuilder, key: private::Key) -> Self::Capabilities;
    #[doc(hidden)]
    fn validate(
        capabilities: &Self::Capabilities,
        program: &Arc<()>,
    ) -> Result<(), ProgramBuildError>;
    #[doc(hidden)]
    fn control(
        capabilities: &Self::Capabilities,
        runtime: ControlledRuntimeBuilder,
        key: private::Key,
    ) -> ControlledRuntimeBuilder;
}

/// A declared finite effect boundary.
pub struct Effect<D: EffectDescriptor>(PhantomData<fn() -> D>);
/// A declared source boundary, including any built-in Layers.
pub struct Source<S: SourceDescriptor>(PhantomData<fn() -> S>);

pub(crate) fn declare<R: Requirements>(builder: &mut ProgramBuilder) -> Capabilities<R> {
    R::declare(builder, private::Key)
}

impl<D: EffectDescriptor> private::Requirement for Effect<D> {}
impl<D: EffectDescriptor> Requirements for Effect<D> {
    type Capabilities = EffectCapability<D>;
    fn declare(builder: &mut ProgramBuilder, _: private::Key) -> Self::Capabilities {
        builder.effect::<D>()
    }
    fn validate(cap: &Self::Capabilities, program: &Arc<()>) -> Result<(), ProgramBuildError> {
        if cap.token.belongs_to(program) {
            Ok(())
        } else {
            Err(ProgramBuildError::ForeignCapability {
                descriptor_type: std::any::type_name::<D>(),
            })
        }
    }
    fn control(
        _: &Self::Capabilities,
        runtime: ControlledRuntimeBuilder,
        _: private::Key,
    ) -> ControlledRuntimeBuilder {
        runtime.control_effect::<D>()
    }
}

#[cfg(feature = "fs")]
impl<P: FilePermissions> private::Requirement for File<P> {}
#[cfg(feature = "fs")]
impl<P: FilePermissions> Requirements for File<P> {
    type Capabilities = FileCapability<P>;
    fn declare(builder: &mut ProgramBuilder, _: private::Key) -> Self::Capabilities {
        let token = builder.next_capability();
        if P::READ {
            builder.exact_effect::<FileRead>(&token);
        }
        if P::APPEND {
            builder.exact_effect::<FileAppend>(&token);
        }
        if P::WRITE {
            builder.exact_effect::<FileReplace>(&token);
        }
        FileCapability::new(token)
    }
    fn validate(cap: &Self::Capabilities, program: &Arc<()>) -> Result<(), ProgramBuildError> {
        if cap.token.belongs_to(program) {
            Ok(())
        } else {
            Err(ProgramBuildError::ForeignCapability {
                descriptor_type: std::any::type_name::<File<P>>(),
            })
        }
    }
    fn control(
        cap: &Self::Capabilities,
        mut runtime: ControlledRuntimeBuilder,
        _: private::Key,
    ) -> ControlledRuntimeBuilder {
        if P::READ {
            runtime
                .bindings
                .exact_effects
                .push((cap.token.clone(), TypeId::of::<FileRead>()));
        }
        if P::APPEND {
            runtime
                .bindings
                .exact_effects
                .push((cap.token.clone(), TypeId::of::<FileAppend>()));
        }
        if P::WRITE {
            runtime
                .bindings
                .exact_effects
                .push((cap.token.clone(), TypeId::of::<FileReplace>()));
        }
        runtime
    }
}
impl<S: SourceDescriptor> private::Requirement for Source<S> {}
impl<S: SourceDescriptor> Requirements for Source<S> {
    type Capabilities = SourceCapability<S>;
    fn declare(builder: &mut ProgramBuilder, _: private::Key) -> Self::Capabilities {
        builder.source::<S>()
    }
    fn validate(cap: &Self::Capabilities, program: &Arc<()>) -> Result<(), ProgramBuildError> {
        if cap.token.belongs_to(program) {
            Ok(())
        } else {
            Err(ProgramBuildError::ForeignCapability {
                descriptor_type: std::any::type_name::<S>(),
            })
        }
    }
    fn control(
        cap: &Self::Capabilities,
        mut runtime: ControlledRuntimeBuilder,
        _: private::Key,
    ) -> ControlledRuntimeBuilder {
        if S::__samara_terminal_stream_item_type_id(source_plan_private::LOWER_TOKEN).is_some() {
            runtime.bindings.exact_sources.push(cap.token.clone());
        } else {
            runtime
                .bindings
                .sources
                .insert(S::__samara_terminal_type_id(
                    source_plan_private::LOWER_TOKEN,
                ));
        }
        runtime
    }
}
impl private::Requirement for () {}
impl Requirements for () {
    type Capabilities = ();
    fn declare(_: &mut ProgramBuilder, _: private::Key) {}
    fn validate(_: &(), _: &Arc<()>) -> Result<(), ProgramBuildError> {
        Ok(())
    }
    fn control(
        _: &(),
        runtime: ControlledRuntimeBuilder,
        _: private::Key,
    ) -> ControlledRuntimeBuilder {
        runtime
    }
}

/// Live implementations matching a Program's requirement shape position by position.
///
/// A source driver handles the terminal descriptor beneath its declared Layers.
/// Use `MpscReceiver` for an exact channel resource and `StdinDriver` for stdin.
/// With the `fs` feature, one `FileDriver` supplies all operations of a `File<P>`.
/// Binding is private; supply values through [`RuntimeSetup::with`].
/// This trait is sealed. Implement [`EffectDriver`] or [`SourceDriver`] for
/// application-defined Drivers.
///
/// ```compile_fail
/// use samara::*;
/// fn bypass(capability: &EffectCapability<ReadSystemTime>) {
///     <SystemClockDriver as Drivers<Effect<ReadSystemTime>>>::bind(
///         SystemClockDriver, capability,
///     );
/// }
/// ```
#[allow(private_bounds)] // Sealed marker; registry access stays inside the crate.
pub trait Drivers<R: Requirements>: private::Driver<R> {}
impl<R: Requirements, T: private::Driver<R>> Drivers<R> for T {}

impl<D: EffectDescriptor, T: EffectDriver<D>> private::Driver<Effect<D>> for T {
    fn bind(self, _: &EffectCapability<D>, bindings: &mut live_runtime::LiveBindings) {
        bindings.bind_effect::<D, T>(self);
    }
}
#[cfg(feature = "fs")]
impl<P: FilePermissions> private::Driver<File<P>> for FileDriver {
    fn bind(self, cap: &FileCapability<P>, bindings: &mut live_runtime::LiveBindings) {
        if P::READ {
            bindings.bind_exact_effect::<FileRead, _>(&cap.token, self.clone());
        }
        if P::APPEND {
            bindings.bind_exact_effect::<FileAppend, _>(&cap.token, self.clone());
        }
        if P::WRITE {
            bindings.bind_exact_effect::<FileReplace, _>(&cap.token, self);
        }
    }
}
/// A capability value, or a tuple of them, that live Drivers can be supplied for.
///
/// Implemented for every shape `Program::builder` hands out. Sealed. Pass a
/// reference to [`RuntimeSetup::with`].
#[allow(private_bounds)]
pub trait Capability: private::CapabilityValue + Clone + Send + 'static {
    /// The requirement shape these capability values were issued for.
    type Requires: Requirements<Capabilities = Self>;
}
impl<D: EffectDescriptor> private::CapabilityValue for EffectCapability<D> {}
impl<D: EffectDescriptor> Capability for EffectCapability<D> {
    type Requires = Effect<D>;
}
impl<S: SourceDescriptor> private::CapabilityValue for SourceCapability<S> {}
impl<S: SourceDescriptor> Capability for SourceCapability<S> {
    type Requires = Source<S>;
}
#[cfg(feature = "fs")]
impl<P: FilePermissions> private::CapabilityValue for FileCapability<P> {}
#[cfg(feature = "fs")]
impl<P: FilePermissions> Capability for FileCapability<P> {
    type Requires = File<P>;
}
impl private::CapabilityValue for () {}
impl Capability for () {
    type Requires = ();
}

/// A type-wide terminal source Driver used in a requirement tuple.
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
    fn bind(self, _: &SourceCapability<S>, bindings: &mut live_runtime::LiveBindings) {
        bindings.bind_source::<S::Terminal, T>(self.0);
    }
}
impl private::Driver<()> for () {
    fn bind(self, _: &(), _: &mut live_runtime::LiveBindings) {}
}
macro_rules! tuples {
    ($(($($r:ident $d:ident $i:tt),+))+) => {$(
        impl<$($r: Requirements),+> private::Requirement for ($($r,)+) {}
        impl<$($r: Requirements),+> Requirements for ($($r,)+) {
            type Capabilities = ($(Capabilities<$r>,)+);
            fn declare(builder: &mut ProgramBuilder, _: private::Key) -> Self::Capabilities {
                ($($r::declare(builder, private::Key),)+)
            }
            fn validate(caps: &Self::Capabilities, program: &Arc<()>) -> Result<(), ProgramBuildError> {
                $($r::validate(&caps.$i, program)?;)+ Ok(())
            }
            fn control(caps: &Self::Capabilities, runtime: ControlledRuntimeBuilder, _: private::Key) -> ControlledRuntimeBuilder {
                $(let runtime = $r::control(&caps.$i, runtime, private::Key);)+ runtime
            }
        }
        impl<$($r: Capability),+> private::CapabilityValue for ($($r,)+) {}
        impl<$($r: Capability),+> Capability for ($($r,)+) {
            type Requires = ($($r::Requires,)+);
        }
        impl<$($r: Requirements, $d: Drivers<$r>),+> private::Driver<($($r,)+)> for ($($d,)+) {
            fn bind(self, caps: &Capabilities<($($r,)+)>, bindings: &mut live_runtime::LiveBindings) {
                $(private::Driver::bind(self.$i, &caps.$i, bindings);)+
            }
        }
    )+};
}
tuples! {
    (R1 D1 0)
    (R1 D1 0, R2 D2 1)
    (R1 D1 0, R2 D2 1, R3 D3 2)
    (R1 D1 0, R2 D2 1, R3 D3 2, R4 D4 3)
    (R1 D1 0, R2 D2 1, R3 D3 2, R4 D4 3, R5 D5 4)
    (R1 D1 0, R2 D2 1, R3 D3 2, R4 D4 3, R5 D5 4, R6 D6 5)
    (R1 D1 0, R2 D2 1, R3 D3 2, R4 D4 3, R5 D5 4, R6 D6 5, R7 D7 6)
    (R1 D1 0, R2 D2 1, R3 D3 2, R4 D4 3, R5 D5 4, R6 D6 5, R7 D7 6, R8 D8 7)
}

/// An exact, one-shot channel receiver for a source tuple position.
pub struct MpscReceiver<T>(tokio::sync::mpsc::Receiver<T>);
impl<T> MpscReceiver<T> {
    /// Transfers a receiver into the Program's corresponding source binding.
    pub fn new(receiver: tokio::sync::mpsc::Receiver<T>) -> Self {
        Self(receiver)
    }
}
impl<S: StreamSourceDescriptor> private::Driver<Source<S>> for MpscReceiver<S::StreamItem> {
    fn bind(self, cap: &SourceCapability<S>, bindings: &mut live_runtime::LiveBindings) {
        bindings.bind_mpsc(cap, self.0);
    }
}

/// Binds the corresponding standard-input capability to the process stdin reader.
///
/// Supply this value for a `Source<StdinLines>` capability, or a composed source
/// with that terminal descriptor, through [`RuntimeSetup::with`]. Multiple
/// process-input bindings or a competing type-wide Driver fail runtime build.
/// Only one active subscription may read stdin across all runtimes. A competing
/// reader faults the runtime; application code must not read stdin concurrently.
/// Removing the subscription or shutting down interrupts and joins the reader.
/// A later subscription resumes at stdin's current position.
#[cfg(unix)]
pub struct StdinDriver;
#[cfg(unix)]
impl<S: SourceDescriptor<Terminal = StdinLines>> private::Driver<Source<S>> for StdinDriver {
    fn bind(self, cap: &SourceCapability<S>, bindings: &mut live_runtime::LiveBindings) {
        bindings.bind_stdin_capability(cap);
    }
}

/// A live Program awaiting Drivers for its declared capabilities.
///
/// Name a capability, or a tuple of them, with [`Self::with`] and supply its
/// Drivers, in any order, then [`Self::build`]:
///
/// ```
/// use samara::prelude::*;
/// let (program, (clock,)) = Program::builder::<(Effect<ReadSystemTime>,)>();
/// let runtime = Runtime::builder(program.build().unwrap())
///     .with(&clock)
///     .supplied_by(SystemClockDriver)
///     .build();
/// assert!(runtime.is_ok());
/// ```
///
/// A Driver of the wrong type for its capability does not compile:
///
/// ```compile_fail
/// use samara::prelude::*;
/// let (program, (clock,)) = Program::builder::<(Effect<ReadSystemTime>,)>();
/// Runtime::builder(program.build().unwrap()).with(&clock).supplied_by(samara::HttpDriver::new());
/// ```
///
/// A capability must be given its Drivers before anything else happens:
///
/// ```compile_fail
/// use samara::prelude::*;
/// let (program, (clock,)) = Program::builder::<(Effect<ReadSystemTime>,)>();
/// Runtime::builder(program.build().unwrap()).with(&clock).build();
/// ```
///
/// A capability left without a Driver, or given two, fails `build`, even for
/// a boundary first used after a later Message:
///
/// ```
/// use samara::prelude::*;
/// let (program, _) = Program::builder::<(Effect<ReadSystemTime>,)>();
/// let error = Runtime::builder(program.build().unwrap()).build().err().unwrap();
/// assert!(error.to_string().contains("ReadSystemTime"));
/// ```
///
/// Bindings can only be added this way:
///
/// ```compile_fail
/// use samara::prelude::*;
/// let (program, ()) = Program::builder::<()>();
/// Runtime::builder(program.build().unwrap()).bind_effect::<ReadSystemTime, _>(SystemClockDriver);
/// ```
#[must_use = "supply Drivers with with(&capability).supplied_by(driver) and then build()"]
pub struct RuntimeSetup<R: Requirements> {
    pub(crate) builder: RuntimeBuilder,
    pub(crate) rejected: Option<RuntimeError>,
    pub(crate) requirements: PhantomData<fn() -> R>,
}
impl<R: Requirements> RuntimeSetup<R> {
    /// Names the capabilities whose Drivers are supplied next.
    ///
    /// Takes one capability or a tuple of them, borrowed. Order across calls
    /// does not matter: a Driver goes to the capability value named here,
    /// which is the same value a Component was registered with.
    pub fn with<C: Capability>(self, capabilities: &C) -> RuntimeSupply<'_, R, C> {
        RuntimeSupply {
            setup: self,
            capabilities,
        }
    }

    /// Validates that every declared capability has exactly one Driver and
    /// builds the runtime.
    ///
    /// A missing, duplicate, conflicting, or foreign binding returns an error
    /// naming the descriptor type, as do foreign capabilities in startup
    /// commands and subscriptions. Call [`Runtime::spawn`] to begin work.
    pub fn build(self) -> Result<Runtime, RuntimeError> {
        match self.rejected {
            Some(error) => Err(error),
            None => self.builder.build(),
        }
    }
}
/// Capabilities named by [`RuntimeSetup::with`], awaiting their Drivers.
#[must_use = "supply the Drivers for these capabilities with supplied_by(...)"]
pub struct RuntimeSupply<'a, R: Requirements, C: Capability> {
    setup: RuntimeSetup<R>,
    capabilities: &'a C,
}
impl<R: Requirements, C: Capability> RuntimeSupply<'_, R, C> {
    /// Supplies the live Drivers for the named capabilities.
    ///
    /// The Driver types are checked against the capability shape here.
    /// Whether every declared capability was supplied exactly once is checked
    /// by [`RuntimeSetup::build`].
    pub fn supplied_by(self, drivers: impl Drivers<C::Requires>) -> RuntimeSetup<R> {
        let mut setup = self.setup;
        match C::Requires::validate(self.capabilities, &setup.builder.program.program) {
            Ok(()) => {
                private::Driver::bind(drivers, self.capabilities, &mut setup.builder.bindings)
            }
            Err(error) => {
                setup
                    .rejected
                    .get_or_insert_with(|| RuntimeError::harness(error.to_string()));
            }
        }
        setup
    }
}
/// A controlled Program awaiting explicit selection of its declared boundaries.
///
/// ```compile_fail
/// use samara::prelude::*;
/// let (program, _) = Program::builder::<(Effect<ReadSystemTime>,)>();
/// ControlledRuntime::builder(program.build().unwrap()).build();
/// ```
#[must_use = "select controlled execution with control_all"]
pub struct ControlledRuntimeSetup<R: Requirements> {
    pub(crate) capabilities: Capabilities<R>,
    pub(crate) builder: ControlledRuntimeBuilder,
}
impl<R: Requirements> ControlledRuntimeSetup<R> {
    /// Controls every declared effect and source, preserving exact resource identities.
    pub fn control_all(self) -> ControlledRuntimeBuilder {
        R::control(&self.capabilities, self.builder, private::Key)
    }
}
impl ControlledRuntimeSetup<()> {
    /// Builds a Program with no world boundaries.
    pub fn build(self) -> Result<ControlledRuntime, RuntimeError> {
        self.control_all().build()
    }
}

/// Registration has not received its declared capability tuple yet.
pub struct MissingCapabilities;
/// Registration has received its declared capability tuple.
pub struct SuppliedCapabilities<T>(T);
/// A Component registration, finalized only after its dependencies are supplied.
///
/// ```compile_fail
/// use samara::prelude::*;
/// struct Clock;
/// impl Component for Clock {
///     type Config = (); type Requires = (Effect<ReadSystemTime>,);
///     type Model = (); type Message = ();
///     fn init(_: &(), _: &Capabilities<Self::Requires>) -> Init<(), ()> { Init::default() }
///     fn update(_: &(), _: &Capabilities<Self::Requires>, _: &mut (), _: ()) -> Command<()> { Command::none() }
/// }
/// let (mut program, _) = Program::builder::<(Effect<ReadSystemTime>,)>();
/// program.component::<Clock>(()).finish();
/// ```
///
/// ```compile_fail
/// use samara::prelude::*;
/// struct Clock;
/// impl Component for Clock {
///     type Config = (); type Requires = (Effect<ReadSystemTime>,);
///     type Model = (); type Message = ();
///     fn init(_: &(), _: &Capabilities<Self::Requires>) -> Init<(), ()> { Init::default() }
///     fn update(_: &(), _: &Capabilities<Self::Requires>, _: &mut (), _: ()) -> Command<()> { Command::none() }
/// }
/// let (mut program, http) = Program::builder::<(Effect<HttpRequest>,)>();
/// program.component::<Clock>(()).with_capabilities(&http);
/// ```
#[must_use = "finish component registration with with_capabilities(...).finish()"]
pub struct ComponentRegistration<'a, R: Requirements, C: Component, State = MissingCapabilities> {
    program: &'a mut ProgramBuilder<R>,
    name: Option<Arc<str>>,
    config: C::Config,
    state: State,
}
impl<'a, R: Requirements, C: Component> ComponentRegistration<'a, R, C> {
    pub(crate) fn new(program: &'a mut ProgramBuilder<R>, config: C::Config) -> Self {
        Self {
            program,
            name: None,
            config,
            state: MissingCapabilities,
        }
    }
    /// Supplies the exact capability tuple declared by the Component.
    ///
    /// Borrowed, so the same values can later be named to
    /// [`RuntimeSetup::with`] when Drivers are supplied.
    pub fn with_capabilities(
        self,
        caps: &Capabilities<C::Requires>,
    ) -> ComponentRegistration<'a, R, C, SuppliedCapabilities<Capabilities<C::Requires>>> {
        ComponentRegistration {
            program: self.program,
            name: self.name,
            config: self.config,
            state: SuppliedCapabilities(caps.clone()),
        }
    }
}
impl<R: Requirements, C: Component, State> ComponentRegistration<'_, R, C, State> {
    /// Sets a diagnostic label. Names need not be unique and never affect routing
    /// or execution order. Without this, diagnostics use the Component type name.
    pub fn name(mut self, name: impl Into<Arc<str>>) -> Self {
        self.name = Some(name.into());
        self
    }
}
impl<R: Requirements, C: Component<Requires = ()>> ComponentRegistration<'_, R, C> {
    /// Registers a Component with no world capabilities.
    pub fn finish(self) -> Result<ComponentRef<C>, ProgramBuildError> {
        self.with_capabilities(&()).finish()
    }
}
impl<R: Requirements, C: Component>
    ComponentRegistration<'_, R, C, SuppliedCapabilities<Capabilities<C::Requires>>>
{
    /// Validates provenance, initializes the Component and registers it atomically.
    pub fn finish(self) -> Result<ComponentRef<C>, ProgramBuildError> {
        self.program
            .register::<C>(self.name, self.config, self.state.0)
    }
}

/// A reusable graph fragment wired with capabilities supplied by its enclosing Program.
pub trait Assembly {
    /// Settings and connections supplied by the caller.
    type Config;
    /// What the caller receives: a [`Connector`] for an Assembly with one
    /// door, a struct naming several, or `()` when nobody sends to it.
    type Exports;
    /// The capability tuple required by its internal Components.
    type Requires: Requirements;
    /// Installs Components using the supplied capabilities. Does not declare new boundaries.
    fn install<R: Requirements>(
        program: &mut ProgramBuilder<R>,
        config: Self::Config,
        capabilities: Capabilities<Self::Requires>,
    ) -> Result<Self::Exports, ProgramBuildError>;
}
/// An installed Assembly's exports. Runtime obligations remain in the Program.
pub struct InstalledAssembly<A: Assembly> {
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
    ///     type Requires = ();
    ///     fn install<R: Requirements>(
    ///         _: &mut ProgramBuilder<R>,
    ///         _: (),
    ///         _: Capabilities<()>,
    ///     ) -> Result<Doors, ProgramBuildError> {
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
#[must_use = "finish Assembly installation with with_capabilities(...).finish()"]
pub struct AssemblyRegistration<'a, R: Requirements, A: Assembly, State = MissingCapabilities> {
    program: &'a mut ProgramBuilder<R>,
    config: A::Config,
    state: State,
}
impl<R: Requirements> ProgramBuilder<R> {
    /// Starts installing an Assembly with its settings and connections.
    pub fn install<A: Assembly>(&mut self, config: A::Config) -> AssemblyRegistration<'_, R, A> {
        AssemblyRegistration {
            program: self,
            config,
            state: MissingCapabilities,
        }
    }
}
impl<'a, R: Requirements, A: Assembly> AssemblyRegistration<'a, R, A> {
    /// Supplies the capabilities required by the Assembly, borrowed.
    pub fn with_capabilities(
        self,
        caps: &Capabilities<A::Requires>,
    ) -> AssemblyRegistration<'a, R, A, SuppliedCapabilities<Capabilities<A::Requires>>> {
        AssemblyRegistration {
            program: self.program,
            config: self.config,
            state: SuppliedCapabilities(caps.clone()),
        }
    }
}
impl<R: Requirements, A: Assembly<Requires = ()>> AssemblyRegistration<'_, R, A> {
    /// Installs an Assembly requiring no world capabilities.
    pub fn finish(self) -> Result<InstalledAssembly<A>, ProgramBuildError> {
        self.with_capabilities(&()).finish()
    }
}
impl<R: Requirements, A: Assembly>
    AssemblyRegistration<'_, R, A, SuppliedCapabilities<Capabilities<A::Requires>>>
{
    /// Validates provenance and installs the Assembly. Discard the ProgramBuilder on failure.
    pub fn finish(self) -> Result<InstalledAssembly<A>, ProgramBuildError> {
        A::Requires::validate(&self.state.0, &self.program.program)?;
        let exports = A::install(self.program, self.config, self.state.0)?;
        Ok(InstalledAssembly { exports })
    }
}
/// Optional default live implementations for a library Assembly.
pub trait VendorsDrivers: Assembly {
    /// Live implementation settings, independent of [`Assembly::Config`].
    /// Use `()` when the vendored Drivers have no settings.
    type DriverConfig;
    /// The tuple matching the Assembly's requirements.
    type Drivers: Drivers<Self::Requires>;
    /// Consumes live settings to construct a Program's matching driver tuple.
    /// Controlled execution does not need these settings or implementations.
    fn drivers(config: Self::DriverConfig) -> Self::Drivers;
}
