//! Typed declaration and binding of a closed Program's world boundaries.
use super::*;

mod private {
    pub trait Requirement {}
    pub trait Driver<R> {}
    pub struct Key;
}

/// The capability values corresponding to a requirement shape.
pub type Capabilities<R> = <R as Requirements>::Capabilities;

/// A finite requirement shape: `Effect<D>`, `Source<S>`, `()`, or nested tuples.
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
pub trait Drivers<R: Requirements>: private::Driver<R> {
    #[doc(hidden)]
    fn bind(self, capabilities: &Capabilities<R>, runtime: RuntimeBuilder) -> RuntimeBuilder;
}
impl<D: EffectDescriptor, T: EffectDriver<D>> private::Driver<Effect<D>> for T {}
impl<D: EffectDescriptor, T: EffectDriver<D>> Drivers<Effect<D>> for T {
    fn bind(self, _: &EffectCapability<D>, runtime: RuntimeBuilder) -> RuntimeBuilder {
        runtime.bind_effect::<D, T>(self)
    }
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
}
impl<S: SourceDescriptor, T: SourceDriver<S::Terminal>> Drivers<Source<S>> for DrivenSource<T> {
    fn bind(self, _: &SourceCapability<S>, runtime: RuntimeBuilder) -> RuntimeBuilder {
        runtime.bind_source::<S::Terminal, T>(self.0)
    }
}
impl private::Driver<()> for () {}
impl Drivers<()> for () {
    fn bind(self, _: &(), runtime: RuntimeBuilder) -> RuntimeBuilder {
        runtime
    }
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
        impl<$($r: Requirements, $d: Drivers<$r>),+> private::Driver<($($r,)+)> for ($($d,)+) {}
        impl<$($r: Requirements, $d: Drivers<$r>),+> Drivers<($($r,)+)> for ($($d,)+) {
            fn bind(self, caps: &Capabilities<($($r,)+)>, runtime: RuntimeBuilder) -> RuntimeBuilder {
                $(let runtime = self.$i.bind(&caps.$i, runtime);)+ runtime
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
impl<S: StreamSourceDescriptor> private::Driver<Source<S>> for MpscReceiver<S::StreamItem> {}
impl<S: StreamSourceDescriptor> Drivers<Source<S>> for MpscReceiver<S::StreamItem> {
    fn bind(self, cap: &SourceCapability<S>, runtime: RuntimeBuilder) -> RuntimeBuilder {
        runtime.bind_mpsc(cap, self.0)
    }
}

/// Binds the corresponding standard-input capability to the process stdin reader.
#[cfg(unix)]
pub struct StdinDriver;
#[cfg(unix)]
impl<S: SourceDescriptor<Terminal = StdinLines>> private::Driver<Source<S>> for StdinDriver {}
#[cfg(unix)]
impl<S: SourceDescriptor<Terminal = StdinLines>> Drivers<Source<S>> for StdinDriver {
    fn bind(self, cap: &SourceCapability<S>, mut runtime: RuntimeBuilder) -> RuntimeBuilder {
        runtime.bindings.bind_stdin_capability(cap);
        runtime
    }
}

/// A live Program awaiting its complete driver tuple.
///
/// Missing bindings cannot reach runtime construction, even for boundaries
/// first used after a later Message:
///
/// ```compile_fail
/// use samara::prelude::*;
/// let (program, _) = Program::builder::<(Effect<ReadSystemTime>,)>();
/// Runtime::builder(program.build().unwrap()).build();
/// ```
///
/// The tuple must have every required position and the correct Driver types:
///
/// ```compile_fail
/// use samara::prelude::*;
/// let (program, _) = Program::builder::<(Effect<ReadSystemTime>,)>();
/// Runtime::builder(program.build().unwrap()).with_drivers(());
/// ```
///
/// ```compile_fail
/// use samara::prelude::*;
/// let (program, _) = Program::builder::<(Effect<ReadSystemTime>,)>();
/// Runtime::builder(program.build().unwrap()).with_drivers((samara::HttpDriver::new(),));
/// ```
#[must_use = "supply the Program's driver tuple with with_drivers"]
pub struct RuntimeSetup<R: Requirements> {
    pub(crate) capabilities: Capabilities<R>,
    pub(crate) builder: RuntimeBuilder,
}
impl<R: Requirements> RuntimeSetup<R> {
    /// Supplies every declared boundary and enables final value-level validation.
    pub fn with_drivers(self, drivers: impl Drivers<R>) -> RuntimeBuilder {
        drivers.bind(&self.capabilities, self.builder)
    }
}
impl RuntimeSetup<()> {
    /// Builds a Program with no world boundaries.
    pub fn build(self) -> Result<Runtime, RuntimeError> {
        self.with_drivers(()).build()
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
    /// Controls every declared effect and source, preserving exact channel identities.
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
/// program.component::<Clock>(ComponentId::new("clock"), ()).finish();
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
/// program.component::<Clock>(ComponentId::new("clock"), ()).with_capabilities(http);
/// ```
#[must_use = "finish component registration with with_capabilities(...).finish()"]
pub struct ComponentRegistration<'a, R: Requirements, C: Component, State = MissingCapabilities> {
    program: &'a mut ProgramBuilder<R>,
    id: ComponentId,
    config: C::Config,
    state: State,
}
impl<'a, R: Requirements, C: Component> ComponentRegistration<'a, R, C> {
    pub(crate) fn new(
        program: &'a mut ProgramBuilder<R>,
        id: ComponentId,
        config: C::Config,
    ) -> Self {
        Self {
            program,
            id,
            config,
            state: MissingCapabilities,
        }
    }
    /// Supplies the exact capability tuple declared by the Component.
    pub fn with_capabilities(
        self,
        caps: Capabilities<C::Requires>,
    ) -> ComponentRegistration<'a, R, C, SuppliedCapabilities<Capabilities<C::Requires>>> {
        ComponentRegistration {
            program: self.program,
            id: self.id,
            config: self.config,
            state: SuppliedCapabilities(caps),
        }
    }
}
impl<R: Requirements, C: Component<Requires = ()>> ComponentRegistration<'_, R, C> {
    /// Registers a Component with no world capabilities.
    pub fn finish(self) -> Result<ComponentRef<C>, ProgramBuildError> {
        self.with_capabilities(()).finish()
    }
}
impl<R: Requirements, C: Component>
    ComponentRegistration<'_, R, C, SuppliedCapabilities<Capabilities<C::Requires>>>
{
    /// Validates provenance, initializes the Component and registers it atomically.
    pub fn finish(self) -> Result<ComponentRef<C>, ProgramBuildError> {
        self.program
            .register::<C>(self.id, self.config, self.state.0)
    }
}

/// A reusable graph fragment wired with capabilities supplied by its enclosing Program.
pub trait Assembly {
    /// Settings and connections supplied by the caller.
    type Config;
    /// The public message vocabulary accepted by its Connector.
    type Accepts: Send + 'static;
    /// Additional exported values, often `()`.
    type Exports;
    /// The capability tuple required by its internal Components.
    type Requires: Requirements;
    /// Installs Components using the supplied capabilities. Does not declare new boundaries.
    fn install<R: Requirements>(
        program: &mut ProgramBuilder<R>,
        config: Self::Config,
        capabilities: Capabilities<Self::Requires>,
    ) -> Result<(Connector<Self::Accepts>, Self::Exports), ProgramBuildError>;
}
/// An Assembly's address and exports. Runtime obligations remain in the Program.
pub struct InstalledAssembly<A: Assembly> {
    connector: Connector<A::Accepts>,
    exports: A::Exports,
}
impl<A: Assembly> InstalledAssembly<A> {
    /// The Assembly's public address.
    pub fn connector(&self) -> &Connector<A::Accepts> {
        &self.connector
    }
    /// Additional exports.
    pub fn exports(&self) -> &A::Exports {
        &self.exports
    }
}
impl<A: Assembly> Addressable for InstalledAssembly<A> {
    type Message = A::Accepts;
    fn to_address(&self) -> Address<A::Accepts> {
        self.connector.to_address()
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
    /// Supplies the capabilities required by the Assembly.
    pub fn with_capabilities(
        self,
        caps: Capabilities<A::Requires>,
    ) -> AssemblyRegistration<'a, R, A, SuppliedCapabilities<Capabilities<A::Requires>>> {
        AssemblyRegistration {
            program: self.program,
            config: self.config,
            state: SuppliedCapabilities(caps),
        }
    }
}
impl<R: Requirements, A: Assembly<Requires = ()>> AssemblyRegistration<'_, R, A> {
    /// Installs an Assembly requiring no world capabilities.
    pub fn finish(self) -> Result<InstalledAssembly<A>, ProgramBuildError> {
        self.with_capabilities(()).finish()
    }
}
impl<R: Requirements, A: Assembly>
    AssemblyRegistration<'_, R, A, SuppliedCapabilities<Capabilities<A::Requires>>>
{
    /// Validates provenance and installs the Assembly. Discard the ProgramBuilder on failure.
    pub fn finish(self) -> Result<InstalledAssembly<A>, ProgramBuildError> {
        A::Requires::validate(&self.state.0, &self.program.program)?;
        let (connector, exports) = A::install(self.program, self.config, self.state.0)?;
        Ok(InstalledAssembly { connector, exports })
    }
}
/// Optional default live implementations for a library Assembly.
pub trait VendorsDrivers: Assembly {
    /// The tuple matching the Assembly's requirements.
    type Drivers: Drivers<Self::Requires>;
    /// Constructs those implementations for use in a Program's driver tuple.
    fn drivers() -> Self::Drivers;
}
