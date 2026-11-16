//! Executable contract for ADR-0008's closed Program assembly, with the
//! capabilities of ADR-0028: each belongs to the Component it was created for.

use std::{
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use samara::prelude::*;

/// Owns capabilities and never uses them: for tests about what a Program
/// obliges its host to supply or control.
struct Holds<C>(std::marker::PhantomData<fn() -> C>);

impl<C: Io> Component for Holds<C> {
    type Io = C;
    type Connections = ();
    type Configuration = ();
    type Model = ();
    type Message = ();

    fn init(_: &Context<Self>) -> Init<(), ()> {
        Init::new(())
    }

    fn update(_: &Context<Self>, (): &mut (), (): ()) -> Command<()> {
        Command::none()
    }
}

/// Capabilities created for a Component of some other Program.
fn from_another_program<C: Io>() -> C {
    let mut foreign = Program::builder();
    foreign
        .configure_component::<Holds<C>>()
        .name("foreign")
        .add(())
        .io
        .clone()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LateEffect(u64);

impl EffectDescriptor for LateEffect {
    type Output = u64;
    type Error = Infallible;
}

#[derive(Debug)]
enum EffectMessage {
    Trigger,
    Finished(u64),
}

struct EffectComponent;

/// Uses its own capability unless a test smuggles another one in here.
#[derive(Default)]
struct EffectComponentConfig {
    smuggled: Option<Effect<LateEffect>>,
}

impl Component for EffectComponent {
    type Io = (Effect<LateEffect>,);
    type Connections = ();
    type Configuration = EffectComponentConfig;
    type Model = Option<u64>;
    type Message = EffectMessage;

    fn init(_: &Context<Self>) -> Init<Self::Model, Self::Message> {
        Init::new(None)
    }

    fn update(
        cx: &Context<Self>,
        model: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        let (own,) = &cx.io;
        match message {
            EffectMessage::Trigger => {
                Effect::perform(cx.config.smuggled.as_ref().unwrap_or(own), LateEffect(41))
                    .producing(|EffectOutcome::Succeeded(value)| EffectMessage::Finished(value))
            }
            EffectMessage::Finished(value) => {
                *model = Some(value);
                Command::none()
            }
        }
    }
}

fn effect_program() -> (Program, InstalledComponent<EffectComponent>) {
    let mut program = Program::builder();
    let component = program
        .configure_component::<EffectComponent>()
        .name("effect")
        .configuration(EffectComponentConfig::default())
        .add(());
    (program.build().expect("valid closed Program"), component)
}

// A capability left without a Driver fails `RuntimeSetup::build`; see
// tests/capability_binding.rs and tests/owned_capabilities.rs.

#[derive(Clone, Debug, PartialEq, Eq)]
struct LateSource;

impl SourceDescriptor for LateSource {
    type Terminal = Self;
    type Item = u64;
    type Error = Infallible;
}

#[derive(Debug)]
enum SourceMessage {
    Enable,
    Event(SourceEvent<u64, Infallible>),
}

struct SourceComponent;

/// Subscribes through its own capability unless a test smuggles another one
/// in here.
struct SourceComponentConfig {
    smuggled: Option<Source<LateSource>>,
    initially_enabled: bool,
}

impl Component for SourceComponent {
    type Io = (Source<LateSource>,);
    type Connections = ();
    type Configuration = SourceComponentConfig;
    type Model = bool;
    type Message = SourceMessage;

    fn init(cx: &Context<Self>) -> Init<Self::Model, Self::Message> {
        Init::new(cx.config.initially_enabled)
    }

    fn update(
        _: &Context<Self>,
        enabled: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        match message {
            SourceMessage::Enable => *enabled = true,
            SourceMessage::Event(_event) => {}
        }
        Command::none()
    }

    fn subscriptions(cx: &Context<Self>, enabled: &Self::Model) -> Subscriptions<Self::Message> {
        let (own,) = &cx.io;
        if *enabled {
            Subscriptions::one(
                Subscription::source_with(
                    cx.config.smuggled.as_ref().unwrap_or(own),
                    LateSource,
                    SourceMessage::Event,
                )
                .key("late"),
            )
        } else {
            Subscriptions::none()
        }
    }
}

fn source_program() -> (Program, InstalledComponent<SourceComponent>) {
    let mut program = Program::builder();
    let component = program
        .configure_component::<SourceComponent>()
        .name("source")
        .configuration(SourceComponentConfig {
            smuggled: None,
            initially_enabled: false,
        })
        .add(());
    (program.build().expect("valid closed Program"), component)
}

// A capability left without a Driver fails `RuntimeSetup::build`; see
// tests/capability_binding.rs and tests/owned_capabilities.rs.

#[test]
fn declared_source_runs_in_controlled_execution() -> Result<(), RuntimeError> {
    let (program, component) = source_program();
    let mut runtime = ControlledRuntime::builder(program).control_all().build()?;

    runtime.send(&component, SourceMessage::Enable)?;
    runtime.run_until_idle()?;
    runtime.emit_source::<SourceComponent, LateSource>(
        &component,
        &SubscriptionId::key("late"),
        SourceEvent::Item(7),
    )?;
    runtime.run_until_idle()?;
    assert!(*runtime.state(&component)?);
    Ok(())
}

/// The Component owns a Source of its own, which the host binds or controls,
/// but subscribes through one from another Program.
fn foreign_source_program(
    initially_enabled: bool,
) -> (Program, InstalledComponent<SourceComponent>) {
    let mut owner = Program::builder();
    let component = owner
        .configure_component::<SourceComponent>()
        .name("foreign-source")
        .configuration(SourceComponentConfig {
            smuggled: Some(from_another_program()),
            initially_enabled,
        })
        .add(());
    (owner.build().expect("valid owner Program"), component)
}

struct CountingSourceDriver {
    starts: Arc<AtomicUsize>,
}

impl SourceDriver<LateSource> for CountingSourceDriver {
    fn run(&self, _descriptor: LateSource, _sink: SourceSink<LateSource>) -> BoxFuture<()> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {})
    }
}

#[test]
fn initially_visible_foreign_source_capability_is_rejected_during_profile_build() {
    let starts = Arc::new(AtomicUsize::new(0));
    let (program, component) = foreign_source_program(true);
    let error = Runtime::builder(program)
        .supply(
            &component.io,
            (samara::DrivenSource::new(CountingSourceDriver {
                starts: starts.clone(),
            }),),
        )
        .build()
        .err()
        .expect("initial foreign Source must fail live build");
    assert!(error.to_string().contains("another Program"));
    assert_eq!(starts.load(Ordering::SeqCst), 0);

    let (program, _) = foreign_source_program(true);
    let error = ControlledRuntime::builder(program)
        .control_all()
        .build()
        .err()
        .expect("initial foreign Source must fail controlled build");
    assert!(error.to_string().contains("another Program"));
}

#[tokio::test]
async fn dynamically_reached_foreign_source_faults_before_terminal_behavior() {
    let (program, component) = foreign_source_program(false);
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
        .build()
        .expect("the owner's declared Source requirement is controlled");
    runtime
        .send(&component, SourceMessage::Enable)
        .expect("the Message itself is valid");
    let error = runtime
        .run_until_idle()
        .expect_err("foreign Source capability must fault before activation");
    assert!(error.to_string().contains("another Program"));

    let starts = Arc::new(AtomicUsize::new(0));
    let (program, component) = foreign_source_program(false);
    let runtime = Runtime::builder(program)
        .supply(
            &component.io,
            (samara::DrivenSource::new(CountingSourceDriver {
                starts: starts.clone(),
            }),),
        )
        .build()
        .expect("the owner's declared Source requirement is bound");
    let handle = runtime.handle(&component).expect("registered Component");
    let mut task = runtime.spawn();
    handle
        .send(SourceMessage::Enable)
        .await
        .expect("the Message itself is admitted");
    let error = task
        .run_forever()
        .await
        .expect_err("foreign Source capability must terminate live observation");
    assert!(error.to_string().contains("another Program"));
    assert_eq!(starts.load(Ordering::SeqCst), 0);
}

struct CountingEffectDriver {
    calls: Arc<AtomicUsize>,
}

impl EffectDriver<LateEffect> for CountingEffectDriver {
    fn execute(&self, descriptor: LateEffect) -> BoxFuture<Result<u64, Infallible>> {
        let calls = self.calls.clone();
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(descriptor.0 + 1)
        })
    }
}

#[test]
fn declared_effect_runs_in_controlled_execution() -> Result<(), RuntimeError> {
    let (program, component) = effect_program();
    let mut runtime = ControlledRuntime::builder(program).control_all().build()?;

    runtime.send(&component, EffectMessage::Trigger)?;
    runtime.run_until_idle()?;
    let pending = runtime.next_effect::<LateEffect>()?;
    assert_eq!(pending.intent, LateEffect(41));
    runtime.complete(pending, EffectOutcome::Succeeded(42))?;
    runtime.run_until_idle()?;
    assert_eq!(runtime.state(&component)?, &Some(42));
    Ok(())
}

#[tokio::test]
async fn declared_effect_runs_in_live_execution() -> Result<(), RuntimeError> {
    let (program, component) = effect_program();
    let calls = Arc::new(AtomicUsize::new(0));
    let runtime = Runtime::builder(program)
        .supply(
            &component.io,
            (CountingEffectDriver {
                calls: calls.clone(),
            },),
        )
        .build()?;
    let handle = runtime.handle(&component)?;
    let runtime = runtime.spawn();

    handle.send(EffectMessage::Trigger).await?;
    let report = runtime.shutdown(Shutdown::Drain).await?;
    assert!(report.is_clean());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    Ok(())
}

struct InitialEffectComponent;

struct InitialEffectComponentConfig {
    effect: Effect<LateEffect>,
}

impl Component for InitialEffectComponent {
    type Io = (Effect<LateEffect>,);
    type Connections = ();
    type Configuration = InitialEffectComponentConfig;
    type Model = ();
    type Message = ();

    fn init(cx: &Context<Self>) -> Init<Self::Model, Self::Message> {
        Init::new(())
            .with_command(Effect::perform(&cx.config.effect, LateEffect(1)).discard_outcome())
    }

    fn update(
        _: &Context<Self>,
        _model: &mut Self::Model,
        _message: Self::Message,
    ) -> Command<Self::Message> {
        Command::none()
    }
}

/// The Component owns an Effect of its own, which the host binds or
/// controls, but its startup command uses one from another Program.
fn initial_foreign_effect_program() -> (Program, InstalledComponent<InitialEffectComponent>) {
    let mut owner = Program::builder();
    let component = owner
        .configure_component::<InitialEffectComponent>()
        .name("initial-foreign-effect")
        .configuration(InitialEffectComponentConfig {
            effect: from_another_program(),
        })
        .add(());
    (
        owner.build().expect("the hidden field is ordinary Rust"),
        component,
    )
}

#[test]
fn initially_visible_foreign_effect_capability_is_rejected_during_profile_build() {
    let calls = Arc::new(AtomicUsize::new(0));
    let (program, component) = initial_foreign_effect_program();
    let live_error = Runtime::builder(program)
        .supply(
            &component.io,
            (CountingEffectDriver {
                calls: calls.clone(),
            },),
        )
        .build()
        .err()
        .expect("initial foreign capability must fail live build");
    assert!(live_error.to_string().contains("another Program"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let controlled_error = ControlledRuntime::builder(initial_foreign_effect_program().0)
        .control_all()
        .build()
        .err()
        .expect("initial foreign capability must fail controlled build");
    assert!(controlled_error.to_string().contains("another Program"));
}

struct UnitComponent;

impl Component for UnitComponent {
    type Io = ();
    type Connections = ();
    type Configuration = ();
    type Model = ();
    type Message = ();

    fn init(_: &Context<Self>) -> Init<Self::Model, Self::Message> {
        Init::new(())
    }

    fn update(
        _: &Context<Self>,
        _model: &mut Self::Model,
        _message: Self::Message,
    ) -> Command<Self::Message> {
        Command::none()
    }
}

struct InitialForeignSend;

struct InitialForeignSendConfig {
    target: ComponentRef<UnitComponent>,
}

impl Component for InitialForeignSend {
    type Io = ();
    type Connections = ();
    type Configuration = InitialForeignSendConfig;
    type Model = ();
    type Message = ();

    fn init(cx: &Context<Self>) -> Init<Self::Model, Self::Message> {
        Init::new(()).with_command(Command::send(&cx.config.target, ()))
    }

    fn update(
        _: &Context<Self>,
        _model: &mut Self::Model,
        _message: Self::Message,
    ) -> Command<Self::Message> {
        Command::none()
    }
}

fn initial_foreign_send_program() -> Program {
    let mut foreign = Program::builder();
    let target = foreign
        .configure_component::<UnitComponent>()
        .name("foreign-target")
        .add(());

    let mut owner = Program::builder();
    owner
        .configure_component::<InitialForeignSend>()
        .name("initial-foreign-send")
        .configuration(InitialForeignSendConfig {
            target: target.into(),
        })
        .add(());
    owner.build().expect("valid owner Program")
}

/// The foreign Port's vocabulary; the request carries its own reply token.
/// Nothing is ever connected to the Port (the point is that it belongs to
/// another Program), so no provider reads the token.
#[allow(dead_code)]
enum ForeignMessage {
    Signal,
    Read(ReplyWith<()>),
}

struct InitialForeignPortSend;

struct InitialForeignPortSendConfig {
    port: Port<ForeignMessage>,
}

impl Component for InitialForeignPortSend {
    type Io = ();
    type Connections = ();
    type Configuration = InitialForeignPortSendConfig;
    type Model = ();
    type Message = ();

    fn init(cx: &Context<Self>) -> Init<Self::Model, Self::Message> {
        Init::new(()).with_command(Command::send(&cx.config.port, ForeignMessage::Signal))
    }

    fn update(
        _: &Context<Self>,
        _model: &mut Self::Model,
        _message: Self::Message,
    ) -> Command<Self::Message> {
        Command::none()
    }
}

struct InitialForeignRequest;

struct InitialForeignRequestConfig {
    port: Port<ForeignMessage>,
}

impl Component for InitialForeignRequest {
    type Io = ();
    type Connections = ();
    type Configuration = InitialForeignRequestConfig;
    type Model = ();
    type Message = ();

    fn init(cx: &Context<Self>) -> Init<Self::Model, Self::Message> {
        Init::new(())
            .with_command(Command::request(&cx.config.port, ForeignMessage::Read).producing(|_| ()))
    }

    fn update(
        _: &Context<Self>,
        _model: &mut Self::Model,
        _message: Self::Message,
    ) -> Command<Self::Message> {
        Command::none()
    }
}

fn initial_foreign_port_program(request: bool) -> Program {
    let mut foreign = Program::builder();
    let port = foreign.add_port::<ForeignMessage>(PortId::new("foreign"));
    let mut owner = Program::builder();
    if request {
        owner
            .configure_component::<InitialForeignRequest>()
            .name("initial-foreign-request")
            .configuration(InitialForeignRequestConfig { port })
            .add(());
    } else {
        owner
            .configure_component::<InitialForeignPortSend>()
            .name("initial-foreign-port-send")
            .configuration(InitialForeignPortSendConfig { port })
            .add(());
    }
    owner.build().expect("valid owner Program")
}

#[test]
fn initially_visible_foreign_component_and_port_capabilities_fail_profile_build() {
    for program in [
        initial_foreign_send_program(),
        initial_foreign_port_program(false),
        initial_foreign_port_program(true),
    ] {
        let error = Runtime::builder(program)
            .build()
            .err()
            .expect("initial foreign connection must fail live build");
        assert!(error.to_string().contains("another Program"));
    }

    for program in [
        initial_foreign_send_program(),
        initial_foreign_port_program(false),
        initial_foreign_port_program(true),
    ] {
        let error = ControlledRuntime::builder(program)
            .control_all()
            .build()
            .err()
            .expect("initial foreign connection must fail controlled build");
        assert!(error.to_string().contains("another Program"));
    }
}

#[test]
fn dynamically_used_foreign_effect_capability_is_rejected_before_controlled_behavior() {
    let mut owner = Program::builder();
    let component = owner
        .configure_component::<EffectComponent>()
        .name("foreign-effect")
        .configuration(EffectComponentConfig {
            smuggled: Some(from_another_program()),
        })
        .add(());
    let program = owner.build().expect("the hidden field is ordinary Rust");
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
        .build()
        .expect("the owning Program's declared requirement is controlled");

    runtime
        .send(&component, EffectMessage::Trigger)
        .expect("the Message itself is valid");
    let error = runtime
        .run_until_idle()
        .expect_err("foreign capability use must fault before interception");
    assert!(error.to_string().contains("another Program"));
    assert!(runtime.next_effect::<LateEffect>().is_err());
}

#[tokio::test]
async fn run_forever_surfaces_dynamic_foreign_capability_before_live_driver() {
    let mut owner = Program::builder();
    let component = owner
        .configure_component::<EffectComponent>()
        .name("foreign-live-effect")
        .configuration(EffectComponentConfig {
            smuggled: Some(from_another_program()),
        })
        .add(());
    let calls = Arc::new(AtomicUsize::new(0));
    let program = owner.build().expect("valid owner Program");
    let runtime = Runtime::builder(program)
        .supply(
            &component.io,
            (CountingEffectDriver {
                calls: calls.clone(),
            },),
        )
        .build()
        .expect("the owner's declared requirement is bound");
    let handle = runtime.handle(&component).expect("registered Component");
    let mut task = runtime.spawn();

    handle
        .send(EffectMessage::Trigger)
        .await
        .expect("the Message itself is admitted");
    let error = task
        .run_forever()
        .await
        .expect_err("foreign capability fault must terminate live observation");
    assert!(error.to_string().contains("another Program"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn controlled_exact_stream_binding_rejects_a_foreign_capability_during_build() {
    let foreign_stream: Source<StreamDescriptor<u64>> = from_another_program();
    let mut owner = Program::builder();
    owner.add_component::<Holds<Source<StreamDescriptor<u64>>>>(());
    let program = owner.build().expect("valid owner Program");
    let error = ControlledRuntime::builder(program)
        .control_all()
        .control_stream(&foreign_stream)
        .build()
        .err()
        .expect("controlled exact binding cannot import a foreign capability");
    assert!(error.to_string().contains("another Program"));
}

#[test]
fn two_exact_stream_capabilities_of_one_type_bind_independently() -> Result<(), RuntimeError> {
    type TwoStreams = (Source<StreamDescriptor<u64>>, Source<StreamDescriptor<u64>>);
    let mut program = Program::builder();
    let holder = program.add_component::<Holds<TwoStreams>>(());
    let program = program.build().expect("valid closed Program");
    let (_first_sender, first_receiver) = tokio::sync::mpsc::channel(1);
    let (_second_sender, second_receiver) = tokio::sync::mpsc::channel(1);

    Runtime::builder(program)
        .supply(
            &holder.io,
            (
                samara::MpscReceiver::new(first_receiver),
                samara::MpscReceiver::new(second_receiver),
            ),
        )
        .build()?;

    let mut program = Program::builder();
    program.add_component::<Holds<TwoStreams>>(());
    let program = program.build().expect("valid closed Program");
    ControlledRuntime::builder(program).control_all().build()?;
    Ok(())
}

#[test]
fn controlled_exact_stream_rejects_duplicate_and_type_wide_ambiguity() {
    fn one_stream() -> (Program, Source<StreamDescriptor<u64>>) {
        let mut program = Program::builder();
        let holder = program.add_component::<Holds<Source<StreamDescriptor<u64>>>>(());
        (
            program.build().expect("valid closed Program"),
            holder.io.clone(),
        )
    }
    let (program, stream) = one_stream();
    let duplicate = ControlledRuntime::builder(program)
        .control_all()
        .control_stream(&stream)
        .build()
        .err()
        .expect("one exact capability cannot be controlled twice");
    assert!(duplicate.to_string().contains("duplicate"));

    let (program, _stream) = one_stream();
    let ambiguous = ControlledRuntime::builder(program)
        .control_all()
        .control_source::<StreamDescriptor<u64>>()
        .build()
        .err()
        .expect("exact and type-wide control cannot both match one capability");
    assert!(ambiguous.to_string().contains("ambiguous"));
}

enum DualStreamMessage {
    First(SourceEvent<u64, Infallible>),
    Second(SourceEvent<u64, Infallible>),
}

#[derive(Debug)]
struct RecordDualStream {
    lane: &'static str,
    value: u64,
}

impl EffectDescriptor for RecordDualStream {
    type Output = ();
    type Error = Infallible;
}

struct RecordDualStreamDriver {
    observed: tokio::sync::mpsc::UnboundedSender<(&'static str, u64)>,
}

impl EffectDriver<RecordDualStream> for RecordDualStreamDriver {
    fn execute(&self, descriptor: RecordDualStream) -> BoxFuture<Result<(), Infallible>> {
        let observed = self.observed.clone();
        Box::pin(async move {
            let _ = observed.send((descriptor.lane, descriptor.value));
            Ok(())
        })
    }
}

struct DualStreamComponent;

struct DualStreamComponentConfig {
    descriptor: StreamDescriptor<u64>,
    observe: bool,
}

/// Two streams of one type, and an Effect that reports what each delivered.
type DualStreamIo = (
    Source<StreamDescriptor<u64>>,
    Source<StreamDescriptor<u64>>,
    Effect<RecordDualStream>,
);

impl Component for DualStreamComponent {
    type Io = DualStreamIo;
    type Connections = ();
    type Configuration = DualStreamComponentConfig;
    type Model = (u64, u64);
    type Message = DualStreamMessage;

    fn init(_: &Context<Self>) -> Init<Self::Model, Self::Message> {
        Init::new((0, 0))
    }

    fn update(
        cx: &Context<Self>,
        model: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        let (_, _, observe) = &cx.io;
        let record = |lane, value| {
            if cx.config.observe {
                Effect::perform(observe, RecordDualStream { lane, value }).discard_outcome()
            } else {
                Command::none()
            }
        };
        match message {
            DualStreamMessage::First(SourceEvent::Item(value)) => {
                model.0 += value;
                record("first", value)
            }
            DualStreamMessage::Second(SourceEvent::Item(value)) => {
                model.1 += value;
                record("second", value)
            }
            DualStreamMessage::First(SourceEvent::Failed(never))
            | DualStreamMessage::Second(SourceEvent::Failed(never)) => match never {},
            DualStreamMessage::First(SourceEvent::Ended)
            | DualStreamMessage::Second(SourceEvent::Ended) => Command::none(),
        }
    }

    fn subscriptions(cx: &Context<Self>, _model: &Self::Model) -> Subscriptions<Self::Message> {
        let (first, second, _) = &cx.io;
        vec![
            Subscription::source_with(
                first,
                cx.config.descriptor.clone(),
                DualStreamMessage::First,
            )
            .key("first"),
            Subscription::source_with(
                second,
                cx.config.descriptor.clone(),
                DualStreamMessage::Second,
            )
            .key("second"),
        ]
        .into()
    }
}

#[test]
fn exact_stream_input_is_disambiguated_by_capability_not_equal_descriptor()
-> Result<(), RuntimeError> {
    let mut program = Program::builder();
    let component = program
        .configure_component::<DualStreamComponent>()
        .name("dual-stream")
        .configuration(DualStreamComponentConfig {
            descriptor: StreamDescriptor::named("same-descriptor"),
            observe: false,
        })
        .add(());
    let (first, second, _) = component.io.clone();
    let mut runtime = ControlledRuntime::builder(program.build().expect("valid Program"))
        .control_all()
        .build()?;

    runtime.emit_stream(&first, 3)?;
    runtime.emit_stream(&second, 7)?;
    runtime.run_until_idle()?;
    assert_eq!(runtime.state(&component)?, &(3, 7));
    Ok(())
}

#[tokio::test]
async fn live_exact_streams_with_equal_descriptors_route_by_capability() -> Result<(), RuntimeError>
{
    let mut program = Program::builder();
    let component = program
        .configure_component::<DualStreamComponent>()
        .name("live-dual-stream")
        .configuration(DualStreamComponentConfig {
            descriptor: StreamDescriptor::named("same-descriptor"),
            observe: true,
        })
        .add(());
    let (first_sender, first_receiver) = tokio::sync::mpsc::channel(1);
    let (second_sender, second_receiver) = tokio::sync::mpsc::channel(1);
    let (observed, mut observations) = tokio::sync::mpsc::unbounded_channel();
    let program = program.build().expect("valid Program");
    let runtime = Runtime::builder(program)
        .supply(
            &component.io,
            (
                samara::MpscReceiver::new(first_receiver),
                samara::MpscReceiver::new(second_receiver),
                RecordDualStreamDriver { observed },
            ),
        )
        .build()?;
    let task = runtime.spawn();

    first_sender
        .send(3)
        .await
        .expect("first receiver remains open");
    second_sender
        .send(7)
        .await
        .expect("second receiver remains open");
    let mut routed = vec![
        tokio::time::timeout(std::time::Duration::from_secs(1), observations.recv())
            .await
            .expect("first routed observation arrived")
            .expect("observation channel remains open"),
        tokio::time::timeout(std::time::Duration::from_secs(1), observations.recv())
            .await
            .expect("second routed observation arrived")
            .expect("observation channel remains open"),
    ];
    routed.sort_unstable();
    assert_eq!(routed, vec![("first", 3), ("second", 7)]);

    let report = task.shutdown(Shutdown::Cancel).await?;
    assert!(report.is_clean());
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct IdentityDecoder;

impl Decoder for IdentityDecoder {
    type Chunk = u64;
    type Frame = u64;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct IncrementDecoder;

impl Decoder for IncrementDecoder {
    type Chunk = u64;
    type Frame = u64;
    type Error = Infallible;
    type State = ();

    fn start(&self) -> Self::State {}

    fn push(
        &self,
        _state: &mut Self::State,
        chunk: Self::Chunk,
    ) -> Result<Vec<Self::Frame>, Self::Error> {
        Ok(vec![chunk + 1])
    }

    fn finish(&self, _state: &mut Self::State) -> Result<Vec<Self::Frame>, Self::Error> {
        Ok(Vec::new())
    }
}

enum FramedStreamMessage {
    Event(SourceEvent<u64, FramedError<Infallible, Infallible>>),
}

type FramedStream = Framed<StreamDescriptor<u64>, IncrementDecoder>;

struct FramedStreamComponent;

struct FramedStreamComponentConfig {
    observe: bool,
}

impl Component for FramedStreamComponent {
    type Io = (Source<FramedStream>, Effect<RecordDualStream>);
    type Connections = ();
    type Configuration = FramedStreamComponentConfig;
    type Model = Vec<u64>;
    type Message = FramedStreamMessage;

    fn init(_: &Context<Self>) -> Init<Self::Model, Self::Message> {
        Init::new(Vec::new())
    }

    fn update(
        cx: &Context<Self>,
        model: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        let (_, observe) = &cx.io;
        let FramedStreamMessage::Event(event) = message;
        match event {
            SourceEvent::Item(value) => {
                model.push(value);
                if cx.config.observe {
                    Effect::perform(
                        observe,
                        RecordDualStream {
                            lane: "framed",
                            value,
                        },
                    )
                    .discard_outcome()
                } else {
                    Command::none()
                }
            }
            SourceEvent::Failed(FramedError::Source(never))
            | SourceEvent::Failed(FramedError::Decode(never)) => match never {},
            SourceEvent::Ended => Command::none(),
        }
    }

    fn subscriptions(cx: &Context<Self>, _model: &Self::Model) -> Subscriptions<Self::Message> {
        let (source, _) = &cx.io;
        Subscriptions::one(
            Subscription::source_with(
                source,
                Framed::new(StreamDescriptor::named("raw"), IncrementDecoder),
                FramedStreamMessage::Event,
            )
            .key("framed-stream"),
        )
    }
}

fn framed_stream_program(
    observe: bool,
) -> (
    Program,
    InstalledComponent<FramedStreamComponent>,
    Source<FramedStream>,
) {
    let mut program = Program::builder();
    let component = program
        .configure_component::<FramedStreamComponent>()
        .name(if observe {
            "live-framed-stream"
        } else {
            "framed-stream"
        })
        .configuration(FramedStreamComponentConfig { observe })
        .add(());
    let source = component.io.0.clone();
    (
        program.build().expect("valid composed stream Program"),
        component,
        source,
    )
}

#[test]
fn exact_stream_bridge_accepts_a_composed_source_capability() -> Result<(), RuntimeError> {
    let (program, component, source) = framed_stream_program(false);
    let (_sender, receiver) = tokio::sync::mpsc::channel(1);
    // The Component also owns its (here unused) observation Effect.
    let (observed, _observations) = tokio::sync::mpsc::unbounded_channel();
    Runtime::builder(program)
        .supply(&source, samara::MpscReceiver::new(receiver))
        .supply(&component.io.1, RecordDualStreamDriver { observed })
        .build()?;

    let (program, component, source) = framed_stream_program(false);
    let mut runtime = ControlledRuntime::builder(program).control_all().build()?;
    runtime.emit_stream(&source, 9)?;
    runtime.close_stream(&source)?;
    runtime.run_until_idle()?;
    assert_eq!(runtime.state(&component)?, &vec![10]);
    Ok(())
}

#[tokio::test]
async fn live_exact_stream_bridge_runs_composed_layers_before_component_mapping()
-> Result<(), RuntimeError> {
    let (program, component, _source) = framed_stream_program(true);
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    let (observed, mut observations) = tokio::sync::mpsc::unbounded_channel();
    let runtime = Runtime::builder(program)
        .supply(
            &component.io,
            (
                samara::MpscReceiver::new(receiver),
                RecordDualStreamDriver { observed },
            ),
        )
        .build()?;
    let task = runtime.spawn();

    sender
        .send(41)
        .await
        .expect("the exact mpsc Source remains active");
    let observation = tokio::time::timeout(std::time::Duration::from_secs(1), observations.recv())
        .await
        .expect("the decoded Component observation arrived")
        .expect("the observation channel remains open");
    assert_eq!(observation, ("framed", 42));

    assert!(task.shutdown(Shutdown::Cancel).await?.is_clean());
    Ok(())
}

struct LateSourceDriver;

impl SourceDriver<LateSource> for LateSourceDriver {
    fn run(&self, _descriptor: LateSource, _sink: SourceSink<LateSource>) -> BoxFuture<()> {
        Box::pin(async {})
    }
}

type ComposedSource = Source<Framed<LateSource, IdentityDecoder>>;

fn composed_source_program() -> (Program, ComposedSource) {
    let mut program = Program::builder();
    let holder = program.add_component::<Holds<ComposedSource>>(());
    (
        program.build().expect("valid composed Source declaration"),
        holder.io.clone(),
    )
}

#[test]
fn composed_source_capability_declares_only_its_terminal_requirement() -> Result<(), RuntimeError> {
    let (program, source) = composed_source_program();
    Runtime::builder(program)
        .supply(&source, samara::DrivenSource::new(LateSourceDriver))
        .build()?;

    ControlledRuntime::builder(composed_source_program().0)
        .control_all()
        .build()?;
    Ok(())
}
