//! Executable contract for ADR-0008's closed Program assembly.

use std::{
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use samara::prelude::*;

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

struct EffectComponent {
    effect: EffectCapability<LateEffect>,
}

impl Component for EffectComponent {
    type Model = Option<u64>;
    type Message = EffectMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(None)
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            EffectMessage::Trigger => {
                Command::effect_with(&self.effect, LateEffect(41), |outcome| match outcome {
                    EffectOutcome::Succeeded(value) => EffectMessage::Finished(value),
                    EffectOutcome::Failed(never) => match never {},
                    EffectOutcome::Cancelled(_) => EffectMessage::Finished(0),
                })
            }
            EffectMessage::Finished(value) => {
                *model = Some(value);
                Command::none()
            }
        }
    }
}

fn effect_program() -> (Program, ComponentRef<EffectComponent>) {
    let mut program = Program::builder();
    let effect = program.effect::<LateEffect>();
    let component = program.component(ComponentId::new("effect"), EffectComponent { effect });
    (program.build().expect("valid closed Program"), component)
}

#[test]
fn later_effect_dependency_fails_live_and_controlled_build_when_unbound() {
    let (program, _) = effect_program();
    let live_error = LiveRuntime::builder(program)
        .build()
        .err()
        .expect("the declared Effect dependency is unbound");
    assert!(live_error.to_string().contains("LateEffect"));

    let (program, _) = effect_program();
    let controlled_error = ControlledRuntime::builder(program)
        .build()
        .err()
        .expect("the declared Effect dependency is uncontrolled");
    assert!(controlled_error.to_string().contains("LateEffect"));
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LateSource;

impl SourceDescriptor for LateSource {
    type Item = u64;
    type Error = Infallible;
}

#[derive(Debug)]
enum SourceMessage {
    Enable,
    Event(SourceEvent<u64, Infallible>),
}

struct SourceComponent {
    source: SourceCapability<LateSource>,
    initially_enabled: bool,
}

impl Component for SourceComponent {
    type Model = bool;
    type Message = SourceMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(self.initially_enabled)
    }

    fn update(&self, enabled: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            SourceMessage::Enable => *enabled = true,
            SourceMessage::Event(_event) => {}
        }
        Command::none()
    }

    fn subscriptions(&self, enabled: &Self::Model) -> Subscriptions<Self::Message> {
        if *enabled {
            Subscriptions::one(Subscription::source_with(
                &self.source,
                SubscriptionId::new("late"),
                LateSource,
                SourceMessage::Event,
            ))
        } else {
            Subscriptions::none()
        }
    }
}

fn source_program() -> (Program, ComponentRef<SourceComponent>) {
    let mut program = Program::builder();
    let source = program.source::<LateSource>();
    let component = program.component(
        ComponentId::new("source"),
        SourceComponent {
            source,
            initially_enabled: false,
        },
    );
    (program.build().expect("valid closed Program"), component)
}

#[test]
fn later_source_dependency_fails_live_and_controlled_build_when_unbound() {
    let (program, _) = source_program();
    let live_error = LiveRuntime::builder(program)
        .build()
        .err()
        .expect("the declared Source dependency is unbound");
    assert!(live_error.to_string().contains("LateSource"));

    let (program, _) = source_program();
    let controlled_error = ControlledRuntime::builder(program)
        .build()
        .err()
        .expect("the declared Source dependency is uncontrolled");
    assert!(controlled_error.to_string().contains("LateSource"));
}

#[test]
fn declared_source_runs_in_controlled_execution() -> Result<(), RuntimeError> {
    let (program, component) = source_program();
    let mut runtime = ControlledRuntime::builder(program)
        .control_source::<LateSource>()
        .build()?;

    runtime.send(&component, SourceMessage::Enable)?;
    runtime.run_until_idle()?;
    runtime.emit_source::<SourceComponent, LateSource>(
        &component,
        &SubscriptionId::new("late"),
        SourceEvent::Item(7),
    )?;
    runtime.run_until_idle()?;
    assert!(*runtime.state(&component)?);
    Ok(())
}

fn foreign_source_program(initially_enabled: bool) -> (Program, ComponentRef<SourceComponent>) {
    let mut foreign = Program::builder();
    let foreign_source = foreign.source::<LateSource>();

    let mut owner = Program::builder();
    let _owner_source = owner.source::<LateSource>();
    let component = owner.component(
        ComponentId::new("foreign-source"),
        SourceComponent {
            source: foreign_source,
            initially_enabled,
        },
    );
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
    let (program, _) = foreign_source_program(true);
    let error = LiveRuntime::builder(program)
        .bind_source::<LateSource, _>(CountingSourceDriver {
            starts: starts.clone(),
        })
        .build()
        .err()
        .expect("initial foreign Source must fail live build");
    assert!(error.to_string().contains("another Program"));
    assert_eq!(starts.load(Ordering::SeqCst), 0);

    let (program, _) = foreign_source_program(true);
    let error = ControlledRuntime::builder(program)
        .control_source::<LateSource>()
        .build()
        .err()
        .expect("initial foreign Source must fail controlled build");
    assert!(error.to_string().contains("another Program"));
}

#[tokio::test]
async fn dynamically_reached_foreign_source_faults_before_terminal_behavior() {
    let (program, component) = foreign_source_program(false);
    let mut runtime = ControlledRuntime::builder(program)
        .control_source::<LateSource>()
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
    let runtime = LiveRuntime::builder(program)
        .bind_source::<LateSource, _>(CountingSourceDriver {
            starts: starts.clone(),
        })
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
    let mut runtime = ControlledRuntime::builder(program)
        .control_effect::<LateEffect>()
        .build()?;

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
    let runtime = LiveRuntime::builder(program)
        .bind_effect::<LateEffect, _>(CountingEffectDriver {
            calls: calls.clone(),
        })
        .build()?;
    let handle = runtime.handle(&component)?;
    let runtime = runtime.spawn();

    handle.send(EffectMessage::Trigger).await?;
    let report = runtime.shutdown(Shutdown::Drain).await?;
    assert!(report.is_clean());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    Ok(())
}

struct InitialEffectComponent {
    effect: EffectCapability<LateEffect>,
}

impl Component for InitialEffectComponent {
    type Model = ();
    type Message = ();

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(()).with_command(Command::effect_discarding_outcome(
            &self.effect,
            LateEffect(1),
        ))
    }

    fn update(&self, _model: &mut Self::Model, _message: Self::Message) -> Command<Self::Message> {
        Command::none()
    }
}

fn initial_foreign_effect_program() -> Program {
    let mut foreign = Program::builder();
    let foreign_effect = foreign.effect::<LateEffect>();

    let mut owner = Program::builder();
    let _owner_effect = owner.effect::<LateEffect>();
    owner.component(
        ComponentId::new("initial-foreign-effect"),
        InitialEffectComponent {
            effect: foreign_effect,
        },
    );
    owner.build().expect("the hidden field is ordinary Rust")
}

#[test]
fn initially_visible_foreign_effect_capability_is_rejected_during_profile_build() {
    let calls = Arc::new(AtomicUsize::new(0));
    let live_error = LiveRuntime::builder(initial_foreign_effect_program())
        .bind_effect::<LateEffect, _>(CountingEffectDriver {
            calls: calls.clone(),
        })
        .build()
        .err()
        .expect("initial foreign capability must fail live build");
    assert!(live_error.to_string().contains("another Program"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let controlled_error = ControlledRuntime::builder(initial_foreign_effect_program())
        .control_effect::<LateEffect>()
        .build()
        .err()
        .expect("initial foreign capability must fail controlled build");
    assert!(controlled_error.to_string().contains("another Program"));
}

struct UnitComponent;

impl Component for UnitComponent {
    type Model = ();
    type Message = ();

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(())
    }

    fn update(&self, _model: &mut Self::Model, _message: Self::Message) -> Command<Self::Message> {
        Command::none()
    }
}

struct InitialForeignSend {
    target: ComponentRef<UnitComponent>,
}

impl Component for InitialForeignSend {
    type Model = ();
    type Message = ();

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(()).with_command(Command::send(self.target.clone(), ()))
    }

    fn update(&self, _model: &mut Self::Model, _message: Self::Message) -> Command<Self::Message> {
        Command::none()
    }
}

fn initial_foreign_send_program() -> Program {
    let mut foreign = Program::builder();
    let target = foreign.component(ComponentId::new("foreign-target"), UnitComponent);

    let mut owner = Program::builder();
    owner.component(
        ComponentId::new("initial-foreign-send"),
        InitialForeignSend { target },
    );
    owner.build().expect("valid owner Program")
}

protocol! {
    type ForeignProtocol => enum ForeignProtocolMessage {
        Signal,
        Read -> (),
    }
}

struct InitialForeignNotification {
    port: Port<ForeignProtocol>,
}

impl Component for InitialForeignNotification {
    type Model = ();
    type Message = ();

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(()).with_command(Command::notify(self.port.clone(), Signal))
    }

    fn update(&self, _model: &mut Self::Model, _message: Self::Message) -> Command<Self::Message> {
        Command::none()
    }
}

struct InitialForeignRequest {
    port: Port<ForeignProtocol>,
}

impl Component for InitialForeignRequest {
    type Model = ();
    type Message = ();

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(()).with_command(Command::request_with(self.port.clone(), Read, |_| ()))
    }

    fn update(&self, _model: &mut Self::Model, _message: Self::Message) -> Command<Self::Message> {
        Command::none()
    }
}

fn initial_foreign_port_program(request: bool) -> Program {
    let mut foreign = Program::builder();
    let port = foreign.port(PortId::new("foreign"));
    let mut owner = Program::builder();
    if request {
        owner.component(
            ComponentId::new("initial-foreign-request"),
            InitialForeignRequest { port },
        );
    } else {
        owner.component(
            ComponentId::new("initial-foreign-notification"),
            InitialForeignNotification { port },
        );
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
        let error = LiveRuntime::builder(program)
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
            .build()
            .err()
            .expect("initial foreign connection must fail controlled build");
        assert!(error.to_string().contains("another Program"));
    }
}

#[test]
fn dynamically_used_foreign_effect_capability_is_rejected_before_controlled_behavior() {
    let mut foreign = Program::builder();
    let foreign_effect = foreign.effect::<LateEffect>();

    let mut owner = Program::builder();
    let _owner_requirement = owner.effect::<LateEffect>();
    let component = owner.component(
        ComponentId::new("foreign-effect"),
        EffectComponent {
            effect: foreign_effect,
        },
    );
    let program = owner.build().expect("the hidden field is ordinary Rust");
    let mut runtime = ControlledRuntime::builder(program)
        .control_effect::<LateEffect>()
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
    let mut foreign = Program::builder();
    let foreign_effect = foreign.effect::<LateEffect>();

    let mut owner = Program::builder();
    let _owner_requirement = owner.effect::<LateEffect>();
    let component = owner.component(
        ComponentId::new("foreign-live-effect"),
        EffectComponent {
            effect: foreign_effect,
        },
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let runtime = LiveRuntime::builder(owner.build().expect("valid owner Program"))
        .bind_effect::<LateEffect, _>(CountingEffectDriver {
            calls: calls.clone(),
        })
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
fn exact_stream_binding_rejects_a_foreign_capability_during_build() {
    let mut foreign = Program::builder();
    let foreign_stream = foreign.source::<StreamDescriptor<u64>>();

    let mut owner = Program::builder();
    let _owner_stream = owner.source::<StreamDescriptor<u64>>();
    let program = owner.build().expect("valid owner Program");
    let (_sender, receiver) = tokio::sync::mpsc::channel(1);

    let error = LiveRuntime::builder(program)
        .bind_mpsc(&foreign_stream, receiver)
        .build()
        .err()
        .expect("an exact binding cannot import a foreign capability");
    assert!(error.to_string().contains("another Program"));

    let mut foreign = Program::builder();
    let foreign_stream = foreign.source::<StreamDescriptor<u64>>();
    let mut owner = Program::builder();
    let _owner_stream = owner.source::<StreamDescriptor<u64>>();
    let program = owner.build().expect("valid owner Program");
    let error = ControlledRuntime::builder(program)
        .control_stream(&foreign_stream)
        .build()
        .err()
        .expect("controlled exact binding cannot import a foreign capability");
    assert!(error.to_string().contains("another Program"));
}

#[test]
fn two_exact_stream_capabilities_of_one_type_bind_independently() -> Result<(), RuntimeError> {
    let mut program = Program::builder();
    let first = program.source::<StreamDescriptor<u64>>();
    let second = program.source::<StreamDescriptor<u64>>();
    let program = program.build().expect("valid closed Program");
    let (_first_sender, first_receiver) = tokio::sync::mpsc::channel(1);
    let (_second_sender, second_receiver) = tokio::sync::mpsc::channel(1);

    LiveRuntime::builder(program)
        .bind_mpsc(&first, first_receiver)
        .bind_mpsc(&second, second_receiver)
        .build()?;

    let mut program = Program::builder();
    let first = program.source::<StreamDescriptor<u64>>();
    let second = program.source::<StreamDescriptor<u64>>();
    let program = program.build().expect("valid closed Program");
    ControlledRuntime::builder(program)
        .control_stream(&first)
        .control_stream(&second)
        .build()?;
    Ok(())
}

#[test]
fn controlled_exact_stream_rejects_duplicate_and_type_wide_ambiguity() {
    let mut program = Program::builder();
    let stream = program.source::<StreamDescriptor<u64>>();
    let program = program.build().expect("valid closed Program");
    let duplicate = ControlledRuntime::builder(program)
        .control_stream(&stream)
        .control_stream(&stream)
        .build()
        .err()
        .expect("one exact capability cannot be controlled twice");
    assert!(duplicate.to_string().contains("duplicate"));

    let mut program = Program::builder();
    let stream = program.source::<StreamDescriptor<u64>>();
    let program = program.build().expect("valid closed Program");
    let ambiguous = ControlledRuntime::builder(program)
        .control_stream(&stream)
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

struct DualStreamComponent {
    first: SourceCapability<StreamDescriptor<u64>>,
    second: SourceCapability<StreamDescriptor<u64>>,
    descriptor: StreamDescriptor<u64>,
    observe: Option<EffectCapability<RecordDualStream>>,
}

impl Component for DualStreamComponent {
    type Model = (u64, u64);
    type Message = DualStreamMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new((0, 0))
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            DualStreamMessage::First(SourceEvent::Item(value)) => {
                model.0 += value;
                self.observe.as_ref().map_or_else(Command::none, |observe| {
                    Command::effect_discarding_outcome(
                        observe,
                        RecordDualStream {
                            lane: "first",
                            value,
                        },
                    )
                })
            }
            DualStreamMessage::Second(SourceEvent::Item(value)) => {
                model.1 += value;
                self.observe.as_ref().map_or_else(Command::none, |observe| {
                    Command::effect_discarding_outcome(
                        observe,
                        RecordDualStream {
                            lane: "second",
                            value,
                        },
                    )
                })
            }
            DualStreamMessage::First(SourceEvent::Failed(never))
            | DualStreamMessage::Second(SourceEvent::Failed(never)) => match never {},
            DualStreamMessage::First(SourceEvent::Ended)
            | DualStreamMessage::Second(SourceEvent::Ended) => Command::none(),
        }
    }

    fn subscriptions(&self, _model: &Self::Model) -> Subscriptions<Self::Message> {
        vec![
            Subscription::source_with(
                &self.first,
                SubscriptionId::new("first"),
                self.descriptor.clone(),
                DualStreamMessage::First,
            ),
            Subscription::source_with(
                &self.second,
                SubscriptionId::new("second"),
                self.descriptor.clone(),
                DualStreamMessage::Second,
            ),
        ]
        .into()
    }
}

#[test]
fn exact_stream_input_is_disambiguated_by_capability_not_equal_descriptor()
-> Result<(), RuntimeError> {
    let mut program = Program::builder();
    let first = program.source::<StreamDescriptor<u64>>();
    let second = program.source::<StreamDescriptor<u64>>();
    let component = program.component(
        ComponentId::new("dual-stream"),
        DualStreamComponent {
            first: first.clone(),
            second: second.clone(),
            descriptor: StreamDescriptor::named("same-descriptor"),
            observe: None,
        },
    );
    let mut runtime = ControlledRuntime::builder(program.build().expect("valid Program"))
        .control_stream(&first)
        .control_stream(&second)
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
    let first = program.source::<StreamDescriptor<u64>>();
    let second = program.source::<StreamDescriptor<u64>>();
    let observe = program.effect::<RecordDualStream>();
    program.component(
        ComponentId::new("live-dual-stream"),
        DualStreamComponent {
            first: first.clone(),
            second: second.clone(),
            descriptor: StreamDescriptor::named("same-descriptor"),
            observe: Some(observe),
        },
    );
    let (first_sender, first_receiver) = tokio::sync::mpsc::channel(1);
    let (second_sender, second_receiver) = tokio::sync::mpsc::channel(1);
    let (observed, mut observations) = tokio::sync::mpsc::unbounded_channel();
    let runtime = LiveRuntime::builder(program.build().expect("valid Program"))
        .bind_mpsc(&first, first_receiver)
        .bind_mpsc(&second, second_receiver)
        .bind_effect::<RecordDualStream, _>(RecordDualStreamDriver { observed })
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

struct FramedStreamComponent {
    source: SourceCapability<FramedStream>,
    observe: Option<EffectCapability<RecordDualStream>>,
}

impl Component for FramedStreamComponent {
    type Model = Vec<u64>;
    type Message = FramedStreamMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(Vec::new())
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        let FramedStreamMessage::Event(event) = message;
        match event {
            SourceEvent::Item(value) => {
                model.push(value);
                self.observe.as_ref().map_or_else(Command::none, |observe| {
                    Command::effect_discarding_outcome(
                        observe,
                        RecordDualStream {
                            lane: "framed",
                            value,
                        },
                    )
                })
            }
            SourceEvent::Failed(FramedError::Source(never))
            | SourceEvent::Failed(FramedError::Decode(never)) => match never {},
            SourceEvent::Ended => Command::none(),
        }
    }

    fn subscriptions(&self, _model: &Self::Model) -> Subscriptions<Self::Message> {
        Subscriptions::one(Subscription::source_with(
            &self.source,
            SubscriptionId::new("framed-stream"),
            Framed::new(StreamDescriptor::named("raw"), IncrementDecoder),
            FramedStreamMessage::Event,
        ))
    }
}

fn framed_stream_program() -> (
    Program,
    ComponentRef<FramedStreamComponent>,
    SourceCapability<FramedStream>,
) {
    let mut program = Program::builder();
    let source = program.source::<FramedStream>();
    let component = program.component(
        ComponentId::new("framed-stream"),
        FramedStreamComponent {
            source: source.clone(),
            observe: None,
        },
    );
    (
        program.build().expect("valid composed stream Program"),
        component,
        source,
    )
}

#[test]
fn exact_stream_bridge_accepts_a_composed_source_capability() -> Result<(), RuntimeError> {
    let (program, _component, source) = framed_stream_program();
    let (_sender, receiver) = tokio::sync::mpsc::channel(1);
    LiveRuntime::builder(program)
        .bind_mpsc(&source, receiver)
        .build()?;

    let (program, component, source) = framed_stream_program();
    let mut runtime = ControlledRuntime::builder(program)
        .control_stream(&source)
        .build()?;
    runtime.emit_stream(&source, 9)?;
    runtime.close_stream(&source)?;
    runtime.run_until_idle()?;
    assert_eq!(runtime.state(&component)?, &vec![10]);
    Ok(())
}

#[tokio::test]
async fn live_exact_stream_bridge_runs_composed_layers_before_component_mapping()
-> Result<(), RuntimeError> {
    let mut program = Program::builder();
    let source = program.source::<FramedStream>();
    let observe = program.effect::<RecordDualStream>();
    program.component(
        ComponentId::new("live-framed-stream"),
        FramedStreamComponent {
            source: source.clone(),
            observe: Some(observe),
        },
    );
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    let (observed, mut observations) = tokio::sync::mpsc::unbounded_channel();
    let runtime = LiveRuntime::builder(program.build().expect("valid composed stream Program"))
        .bind_mpsc(&source, receiver)
        .bind_effect::<RecordDualStream, _>(RecordDualStreamDriver { observed })
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

fn composed_source_program() -> Program {
    let mut program = Program::builder();
    let _source = program.source::<Framed<LateSource, IdentityDecoder>>();
    program.build().expect("valid composed Source declaration")
}

#[test]
fn composed_source_capability_declares_only_its_terminal_requirement() -> Result<(), RuntimeError> {
    LiveRuntime::builder(composed_source_program())
        .bind_source::<LateSource, _>(LateSourceDriver)
        .build()?;

    ControlledRuntime::builder(composed_source_program())
        .control_source::<LateSource>()
        .build()?;
    Ok(())
}
