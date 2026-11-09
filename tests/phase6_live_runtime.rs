type TestProgram = Program<(
    samara::Source<samara::StreamDescriptor<u64>>,
    samara::Effect<Record>,
)>;
type TestProgram2 = Program<(samara::Effect<ProviderSeen>, samara::Effect<ObserveReply>)>;
type TestProgram3 = Program<(samara::Effect<Query>, samara::Effect<ObserveQuery>)>;

use std::{
    convert::Infallible,
    future::pending,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use samara::prelude::*;
use tokio::task::JoinSet;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Record(u64);

impl EffectDescriptor for Record {
    type Output = ();
    type Error = Infallible;
}

#[derive(Clone)]
struct RecordingDriver {
    values: Arc<Mutex<Vec<u64>>>,
    observed: Option<tokio::sync::mpsc::UnboundedSender<u64>>,
}

impl EffectDriver<Record> for RecordingDriver {
    fn execute(&self, descriptor: Record) -> BoxFuture<Result<(), Infallible>> {
        let values = self.values.clone();
        let observed = self.observed.clone();
        Box::pin(async move {
            values.lock().expect("recording lock").push(descriptor.0);
            if let Some(observed) = observed {
                let _ = observed.send(descriptor.0);
            }
            Ok(())
        })
    }
}

#[derive(Debug)]
enum CounterMessage {
    Input(SourceEvent<u64, Infallible>),
    Add(u64),
    Recorded,
}

struct Counter;

struct CounterConfig {
    input: SourceCapability<StreamDescriptor<u64>>,
    input_descriptor: StreamDescriptor<u64>,
    record: EffectCapability<Record>,
}

#[derive(Default)]
struct CounterModel {
    total: u64,
    closed: bool,
}

impl Component for Counter {
    type Requires = ();
    type Config = CounterConfig;
    type Model = CounterModel;
    type Message = CounterMessage;

    fn init(
        _config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
    ) -> Init<Self::Model, Self::Message> {
        Init::new(CounterModel::default())
    }

    fn update(
        config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
        model: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        match message {
            CounterMessage::Input(SourceEvent::Item(value)) | CounterMessage::Add(value) => {
                model.total += value;
                Command::effect_with(&config.record, Record(model.total), |_| {
                    CounterMessage::Recorded
                })
            }
            CounterMessage::Input(SourceEvent::Ended) => {
                model.closed = true;
                Command::none()
            }
            CounterMessage::Input(SourceEvent::Failed(never)) => match never {},
            CounterMessage::Recorded => Command::none(),
        }
    }

    fn subscriptions(
        config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
        model: &Self::Model,
    ) -> Subscriptions<Self::Message> {
        if model.closed {
            Subscriptions::none()
        } else {
            Subscriptions::one(Subscription::source_with(
                &config.input,
                SubscriptionId::new("input"),
                config.input_descriptor.clone(),
                CounterMessage::Input,
            ))
        }
    }
}

fn counter_program(
    input_descriptor: StreamDescriptor<u64>,
) -> (
    TestProgram,
    ComponentRef<Counter>,
    SourceCapability<StreamDescriptor<u64>>,
) {
    let (mut program, (declared_0, declared_1)) = samara::Program::builder::<(
        samara::Source<StreamDescriptor<u64>>,
        samara::Effect<Record>,
    )>();
    let input = declared_0;
    let record = declared_1;
    let counter = program
        .component::<Counter>(
            ComponentId::new("counter"),
            CounterConfig {
                input: input.clone(),
                input_descriptor,
                record,
            },
        )
        .finish()
        .expect("component registration");
    (program.build().expect("valid program"), counter, input)
}

#[tokio::test]
async fn phase6_live_ingress_success_means_accepted() {
    let input = StreamDescriptor::named("counter/input");
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    drop(sender);

    let values = Arc::new(Mutex::new(Vec::new()));
    let (observed, mut observations) = tokio::sync::mpsc::unbounded_channel();
    let (program, counter, _input) = counter_program(input);
    let runtime = Runtime::builder(program)
        .with_drivers((
            samara::MpscReceiver::new(receiver),
            RecordingDriver {
                values: values.clone(),
                observed: Some(observed),
            },
        ))
        .build()
        .expect("valid live bindings");
    let handle = runtime.handle(&counter).expect("registered Component");
    let task = runtime.spawn();

    handle
        .send(CounterMessage::Add(3))
        .await
        .expect("live ingress accepted");
    assert_eq!(observations.recv().await, Some(3));
    let report = task.shutdown(Shutdown::Drain).await.expect("clean drain");

    assert!(report.is_clean());
    assert_eq!(report.pending_now, 0);
    assert_eq!(report.pending_later, 0);
    let values = values.lock().expect("recording lock").clone();
    assert_eq!(values, vec![3]);
    assert!(handle.send(CounterMessage::Add(1)).await.is_err());
}

#[derive(Debug)]
struct Hang;

impl EffectDescriptor for Hang {
    type Output = ();
    type Error = Infallible;
}

struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct HangingDriver {
    entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    dropped: Arc<AtomicBool>,
}

impl EffectDriver<Hang> for HangingDriver {
    fn execute(&self, _descriptor: Hang) -> BoxFuture<Result<(), Infallible>> {
        let entered = self.entered.lock().expect("entry lock").take();
        let dropped = self.dropped.clone();
        Box::pin(async move {
            let _drop = DropFlag(dropped);
            if let Some(entered) = entered {
                let _ = entered.send(());
            }
            pending().await
        })
    }
}

enum CancelMessage {
    Start,
    UnexpectedOutcome,
}

struct CancelProbe;

struct CancelProbeConfig {
    mapper_calls: Arc<AtomicUsize>,
    effect: EffectCapability<Hang>,
}

impl Component for CancelProbe {
    type Requires = ();
    type Config = CancelProbeConfig;
    type Model = ();
    type Message = CancelMessage;

    fn init(
        _config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
    ) -> Init<Self::Model, Self::Message> {
        Init::new(())
    }

    fn update(
        config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
        _model: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        match message {
            CancelMessage::Start => {
                let mapper_calls = config.mapper_calls.clone();
                Command::effect_with(&config.effect, Hang, move |_| {
                    mapper_calls.fetch_add(1, Ordering::SeqCst);
                    CancelMessage::UnexpectedOutcome
                })
            }
            CancelMessage::UnexpectedOutcome => panic!("scope Cancel must not invoke the mapper"),
        }
    }
}

#[tokio::test]
async fn phase6_scope_cancel_drops_effect_without_invoking_mapper() {
    let (mut program, (declared_0,)) = samara::Program::builder::<(samara::Effect<Hang>,)>();
    let mapper_calls = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicBool::new(false));
    let (entered, entered_rx) = tokio::sync::oneshot::channel();
    let effect = declared_0;
    let probe = program
        .component::<CancelProbe>(
            ComponentId::new("cancel-probe"),
            CancelProbeConfig {
                mapper_calls: mapper_calls.clone(),
                effect,
            },
        )
        .finish()
        .expect("component registration");
    let runtime = Runtime::builder(program.build().expect("valid program"))
        .with_drivers((HangingDriver {
            entered: Mutex::new(Some(entered)),
            dropped: dropped.clone(),
        },))
        .build()
        .expect("valid live bindings");
    let handle = runtime.handle(&probe).expect("registered Component");
    let task = runtime.spawn();
    handle.send(CancelMessage::Start).await.expect("accepted");
    entered_rx.await.expect("the hung Driver started");

    let report = task.shutdown(Shutdown::Cancel).await.expect("clean cancel");
    assert!(report.is_clean());
    assert_eq!(report.pending_now, 0);
    assert_eq!(report.pending_later, 0);
    assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(mapper_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn phase6_cancelled_shutdown_future_detaches_no_runtime_work() {
    let (mut program, (declared_0,)) = samara::Program::builder::<(samara::Effect<Hang>,)>();
    let mapper_calls = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicBool::new(false));
    let (entered, entered_rx) = tokio::sync::oneshot::channel();
    let effect = declared_0;
    let probe = program
        .component::<CancelProbe>(
            ComponentId::new("shutdown-future-cancel"),
            CancelProbeConfig {
                mapper_calls: mapper_calls.clone(),
                effect,
            },
        )
        .finish()
        .expect("component registration");
    let runtime = Runtime::builder(program.build().expect("valid program"))
        .with_drivers((HangingDriver {
            entered: Mutex::new(Some(entered)),
            dropped: dropped.clone(),
        },))
        .build()
        .expect("valid live bindings");
    let handle = runtime.handle(&probe).expect("registered Component");
    let task = runtime.spawn();
    handle.send(CancelMessage::Start).await.expect("accepted");
    entered_rx.await.expect("the hung Driver started");

    let shutdown = tokio::spawn(task.shutdown(Shutdown::Drain));
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if handle.send(CancelMessage::Start).await.is_err() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("Drain cutoff closes ingress");

    // Aborting the host future drops RuntimeTask while it is awaiting the
    // owner. Its retained JoinHandle must abort that owner, whose JoinSet in
    // turn aborts the hung Driver rather than detaching it.
    shutdown.abort();
    assert!(
        shutdown
            .await
            .expect_err("host shutdown future aborted")
            .is_cancelled()
    );
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !dropped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("runtime-owned Driver future was dropped");
    assert_eq!(mapper_calls.load(Ordering::SeqCst), 0);
}

struct InitialCancelProbe;

struct InitialCancelProbeConfig {
    mapper_calls: Arc<AtomicUsize>,
    effect: EffectCapability<Hang>,
}

#[tokio::test]
async fn shutdown_escalation_joins_pending_effect_without_mapping() {
    let (mut program, (declared_0,)) = samara::Program::builder::<(samara::Effect<Hang>,)>();
    let mapper_calls = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicBool::new(false));
    let (entered, entered_rx) = tokio::sync::oneshot::channel();
    let effect = declared_0;
    let probe = program
        .component::<CancelProbe>(
            ComponentId::new("escalation-effect"),
            CancelProbeConfig {
                mapper_calls: mapper_calls.clone(),
                effect,
            },
        )
        .finish()
        .expect("component registration");
    let runtime = Runtime::builder(program.build().expect("valid program"))
        .with_drivers((HangingDriver {
            entered: Mutex::new(Some(entered)),
            dropped: dropped.clone(),
        },))
        .build()
        .expect("valid bindings");
    let handle = runtime.handle(&probe).expect("live handle");
    let mut task = runtime.spawn();
    handle.send(CancelMessage::Start).await.expect("accepted");
    entered_rx.await.expect("Driver started");

    task.request_shutdown(Shutdown::Drain);
    task.request_shutdown(Shutdown::Drain);
    assert!(handle.send(CancelMessage::Start).await.is_err());
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(5), task.run_forever())
            .await
            .is_err()
    );
    assert!(!dropped.load(Ordering::SeqCst));

    task.request_shutdown(Shutdown::Cancel);
    task.request_shutdown(Shutdown::Drain);
    let report = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        task.shutdown(Shutdown::Drain),
    )
    .await
    .expect("Cancel cannot be reversed")
    .expect("clean cancellation");
    assert_eq!(report.remaining, 0);
    assert_eq!(report.pending_now, 0);
    assert_eq!(report.pending_later, 0);
    assert!(dropped.load(Ordering::SeqCst), "cleanup was joined");
    assert_eq!(mapper_calls.load(Ordering::SeqCst), 0);
}

impl Component for InitialCancelProbe {
    type Requires = ();
    type Config = InitialCancelProbeConfig;
    type Model = ();
    type Message = ();

    fn init(
        config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
    ) -> Init<Self::Model, Self::Message> {
        let mapper_calls = config.mapper_calls.clone();
        Init::new(()).with_command(Command::effect_with(&config.effect, Hang, move |_| {
            mapper_calls.fetch_add(1, Ordering::SeqCst);
        }))
    }

    fn update(
        _config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
        _model: &mut Self::Model,
        (): (),
    ) -> Command<Self::Message> {
        Command::none()
    }
}

struct CountedHangingDriver {
    calls: Arc<AtomicUsize>,
}

impl EffectDriver<Hang> for CountedHangingDriver {
    fn execute(&self, _descriptor: Hang) -> BoxFuture<Result<(), Infallible>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(pending())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn phase6_cancel_stops_driving_without_mapping_scope_abort() {
    let mapper_calls = Arc::new(AtomicUsize::new(0));
    let driver_calls = Arc::new(AtomicUsize::new(0));
    let (mut program, (declared_0,)) = samara::Program::builder::<(samara::Effect<Hang>,)>();
    let effect = declared_0;
    let _ = program
        .component::<InitialCancelProbe>(
            ComponentId::new("immediate-cancel"),
            InitialCancelProbeConfig {
                mapper_calls: mapper_calls.clone(),
                effect,
            },
        )
        .finish()
        .expect("component registration");
    let runtime = Runtime::builder(program.build().expect("valid program"))
        .with_drivers((CountedHangingDriver {
            calls: driver_calls.clone(),
        },))
        .build()
        .expect("valid live bindings");

    // On a current-thread executor, shutdown closes admission synchronously
    // before the newly spawned owner task can receive its first poll.
    let report = runtime
        .spawn()
        .shutdown(Shutdown::Cancel)
        .await
        .expect("clean immediate cancel");

    assert!(report.is_clean());
    assert_eq!(driver_calls.load(Ordering::SeqCst), 0);
    assert_eq!(mapper_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_escalation_before_owner_poll_cannot_restart_work() {
    let mapper_calls = Arc::new(AtomicUsize::new(0));
    let driver_calls = Arc::new(AtomicUsize::new(0));
    let (mut program, (declared_0,)) = samara::Program::builder::<(samara::Effect<Hang>,)>();
    let effect = declared_0;
    program
        .component::<InitialCancelProbe>(
            ComponentId::new("early-escalation"),
            InitialCancelProbeConfig {
                mapper_calls: mapper_calls.clone(),
                effect,
            },
        )
        .finish()
        .expect("component registration");
    let mut task = Runtime::builder(program.build().expect("valid program"))
        .with_drivers((CountedHangingDriver {
            calls: driver_calls.clone(),
        },))
        .build()
        .expect("valid bindings")
        .spawn();

    task.request_shutdown(Shutdown::Drain);
    task.request_shutdown(Shutdown::Cancel);
    task.request_shutdown(Shutdown::Drain);
    let report = tokio::time::timeout(std::time::Duration::from_secs(1), task.run_forever())
        .await
        .expect("cancellation completes")
        .expect("clean cancellation");
    assert!(report.is_clean());
    assert_eq!(driver_calls.load(Ordering::SeqCst), 0);
    assert_eq!(mapper_calls.load(Ordering::SeqCst), 0);
}

#[derive(Debug)]
struct PanicEffect;

impl EffectDescriptor for PanicEffect {
    type Output = ();
    type Error = Infallible;
}

struct PanickingDriver;

impl EffectDriver<PanicEffect> for PanickingDriver {
    fn execute(&self, _descriptor: PanicEffect) -> BoxFuture<Result<(), Infallible>> {
        Box::pin(async { panic!("driver panic fixture") })
    }
}

struct PanicProbe;

struct PanicProbeConfig {
    effect: EffectCapability<PanicEffect>,
}

impl Component for PanicProbe {
    type Requires = ();
    type Config = PanicProbeConfig;
    type Model = ();
    type Message = ();

    fn init(
        _config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
    ) -> Init<Self::Model, Self::Message> {
        Init::new(())
    }

    fn update(
        config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
        _model: &mut Self::Model,
        (): (),
    ) -> Command<Self::Message> {
        Command::effect_with(&config.effect, PanicEffect, |_| ())
    }
}

#[tokio::test]
async fn phase6_driver_panic_faults_and_cleans_scope() {
    let (mut program, (declared_0,)) = samara::Program::builder::<(samara::Effect<PanicEffect>,)>();
    let effect = declared_0;
    let probe = program
        .component::<PanicProbe>(ComponentId::new("panic-probe"), PanicProbeConfig { effect })
        .finish()
        .expect("component registration");
    let runtime = Runtime::builder(program.build().expect("valid program"))
        .with_drivers((PanickingDriver,))
        .build()
        .expect("valid live bindings");
    let handle = runtime.handle(&probe).expect("registered Component");
    let task = runtime.spawn();
    handle.send(()).await.expect("accepted before the fault");

    let error = task
        .shutdown(Shutdown::Drain)
        .await
        .expect_err("Driver panic must fault the runtime");
    assert_eq!(error.component(), Some(&ComponentId::new("panic-probe")));
    assert!(handle.send(()).await.is_err());
}

#[tokio::test]
async fn shutdown_requests_preserve_completed_fault() {
    let (mut program, (declared_0,)) = samara::Program::builder::<(samara::Effect<PanicEffect>,)>();
    let effect = declared_0;
    let probe = program
        .component::<PanicProbe>(
            ComponentId::new("preserved-fault"),
            PanicProbeConfig { effect },
        )
        .finish()
        .expect("component registration");
    let runtime = Runtime::builder(program.build().expect("valid program"))
        .with_drivers((PanickingDriver,))
        .build()
        .expect("valid bindings");
    let handle = runtime.handle(&probe).expect("live handle");
    let mut task = runtime.spawn();
    handle.send(()).await.expect("accepted");
    let error = task.run_forever().await.expect_err("Driver panicked");
    assert_eq!(error.component(), Some(probe.id()));

    for mode in [Shutdown::Drain, Shutdown::Cancel] {
        task.request_shutdown(mode);
        assert_eq!(task.run_forever().await, Err(error.clone()));
        assert_eq!(handle.send(()).await, Err(error.clone()));
    }
    assert_eq!(task.shutdown(Shutdown::Drain).await, Err(error));
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ScriptedSource {
    generation: u8,
}

impl SourceDescriptor for ScriptedSource {
    type Terminal = Self;
    type Item = u64;
    type Error = &'static str;
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Observation {
    ItemsThenEnded(Vec<u64>),
    Failed(&'static str),
    Ended,
    Timer(u64),
}

#[derive(Clone, Debug)]
struct Observe(Observation);

impl EffectDescriptor for Observe {
    type Output = ();
    type Error = Infallible;
}

struct ObservationDriver {
    observations: tokio::sync::mpsc::UnboundedSender<Observation>,
}

impl EffectDriver<Observe> for ObservationDriver {
    fn execute(&self, descriptor: Observe) -> BoxFuture<Result<(), Infallible>> {
        let observations = self.observations.clone();
        Box::pin(async move {
            let _ = observations.send(descriptor.0);
            Ok(())
        })
    }
}

struct DropCount(Arc<AtomicUsize>);

impl Drop for DropCount {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct HarnessSourceDriver {
    starts: Arc<AtomicUsize>,
    dropped: Arc<AtomicUsize>,
    sinks: tokio::sync::mpsc::UnboundedSender<Arc<SourceSink<ScriptedSource>>>,
}

impl SourceDriver<ScriptedSource> for HarnessSourceDriver {
    fn run(&self, _descriptor: ScriptedSource, sink: SourceSink<ScriptedSource>) -> BoxFuture<()> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        let dropped = self.dropped.clone();
        let _ = self.sinks.send(Arc::new(sink));
        Box::pin(async move {
            let _drop = DropCount(dropped);
            pending().await
        })
    }
}

enum SourceMessage {
    Event(SourceEvent<u64, &'static str>),
    Replace(u8),
    Remove,
    Timer(u64),
    Observed,
}

struct SourceProbe;

struct SourceProbeConfig {
    causal_timer: bool,
    source: SourceCapability<ScriptedSource>,
    observe: EffectCapability<Observe>,
}

struct SourceModel {
    generation: Option<u8>,
    items: Vec<u64>,
}

impl Component for SourceProbe {
    type Requires = ();
    type Config = SourceProbeConfig;
    type Model = SourceModel;
    type Message = SourceMessage;

    fn init(
        _config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
    ) -> Init<Self::Model, Self::Message> {
        Init::new(SourceModel {
            generation: Some(1),
            items: Vec::new(),
        })
    }

    fn update(
        config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
        model: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        match message {
            SourceMessage::Event(SourceEvent::Item(value)) if config.causal_timer => {
                Command::after(
                    std::time::Duration::from_millis(1),
                    SourceMessage::Timer(value),
                )
            }
            SourceMessage::Event(SourceEvent::Item(value)) => {
                model.items.push(value);
                Command::none()
            }
            SourceMessage::Event(SourceEvent::Failed(error)) => {
                Command::effect_with(&config.observe, Observe(Observation::Failed(error)), |_| {
                    SourceMessage::Observed
                })
            }
            SourceMessage::Event(SourceEvent::Ended) => {
                let observation = if model.items.is_empty() {
                    Observation::Ended
                } else {
                    Observation::ItemsThenEnded(model.items.clone())
                };
                Command::effect_with(&config.observe, Observe(observation), |_| {
                    SourceMessage::Observed
                })
            }
            SourceMessage::Replace(generation) => {
                model.generation = Some(generation);
                Command::none()
            }
            SourceMessage::Remove => {
                model.generation = None;
                Command::none()
            }
            SourceMessage::Timer(value) => {
                Command::effect_with(&config.observe, Observe(Observation::Timer(value)), |_| {
                    SourceMessage::Observed
                })
            }
            SourceMessage::Observed => Command::none(),
        }
    }

    fn subscriptions(
        config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
        model: &Self::Model,
    ) -> Subscriptions<Self::Message> {
        model
            .generation
            .map_or_else(Subscriptions::none, |generation| {
                Subscriptions::one(Subscription::source_with(
                    &config.source,
                    SubscriptionId::new("scripted"),
                    ScriptedSource { generation },
                    SourceMessage::Event,
                ))
            })
    }
}

struct SourceFixture {
    runtime: Runtime,
    component: ComponentRef<SourceProbe>,
    observations: tokio::sync::mpsc::UnboundedReceiver<Observation>,
    sinks: tokio::sync::mpsc::UnboundedReceiver<Arc<SourceSink<ScriptedSource>>>,
    starts: Arc<AtomicUsize>,
    dropped: Arc<AtomicUsize>,
}

fn source_fixture(causal_timer: bool) -> SourceFixture {
    let (mut program, (declared_0, declared_1)) =
        samara::Program::builder::<(samara::Source<ScriptedSource>, samara::Effect<Observe>)>();
    let source = declared_0;
    let observe = declared_1;
    let component = program
        .component::<SourceProbe>(
            ComponentId::new("source-probe"),
            SourceProbeConfig {
                causal_timer,
                source,
                observe,
            },
        )
        .finish()
        .expect("component registration");
    let (observation_tx, observations) = tokio::sync::mpsc::unbounded_channel();
    let (sink_tx, sinks) = tokio::sync::mpsc::unbounded_channel();
    let starts = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let runtime = Runtime::builder(program.build().expect("valid source program"))
        .with_drivers((
            samara::DrivenSource::new(HarnessSourceDriver {
                starts: starts.clone(),
                dropped: dropped.clone(),
                sinks: sink_tx,
            }),
            ObservationDriver {
                observations: observation_tx,
            },
        ))
        .build()
        .expect("valid source bindings");
    SourceFixture {
        runtime,
        component,
        observations,
        sinks,
        starts,
        dropped,
    }
}

#[tokio::test]
async fn phase6_source_delivery_preserves_fifo_through_eof() {
    let SourceFixture {
        runtime,
        mut observations,
        mut sinks,
        starts,
        ..
    } = source_fixture(false);
    let task = runtime.spawn();
    let sink = sinks.recv().await.expect("Source started");

    sink.emit(1).await.expect("first item accepted");
    sink.end().await.expect("first terminal accepted");
    assert_eq!(sink.fail("late failure").await, Err(DriverStopped));
    assert_eq!(sink.emit(2).await, Err(DriverStopped));

    assert_eq!(
        observations.recv().await,
        Some(Observation::ItemsThenEnded(vec![1]))
    );
    let report = task.shutdown(Shutdown::Drain).await.expect("clean drain");
    assert!(report.is_clean());
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    assert!(observations.try_recv().is_err());
}

#[tokio::test]
async fn phase6_source_first_terminal_wins() {
    let SourceFixture {
        runtime,
        mut observations,
        mut sinks,
        ..
    } = source_fixture(false);
    let task = runtime.spawn();
    let sink = sinks.recv().await.expect("Source started");

    sink.fail("first failure")
        .await
        .expect("first terminal accepted");
    assert_eq!(sink.end().await, Err(DriverStopped));
    assert_eq!(sink.fail("late failure").await, Err(DriverStopped));
    assert_eq!(
        observations.recv().await,
        Some(Observation::Failed("first failure"))
    );

    assert!(
        task.shutdown(Shutdown::Drain)
            .await
            .expect("clean drain")
            .is_clean()
    );
    assert!(observations.try_recv().is_err());
}

struct SilentSourceDriver {
    starts: Arc<AtomicUsize>,
}

impl SourceDriver<ScriptedSource> for SilentSourceDriver {
    fn run(&self, _descriptor: ScriptedSource, _sink: SourceSink<ScriptedSource>) -> BoxFuture<()> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {})
    }
}

#[tokio::test]
async fn phase6_silent_source_return_ends_once() {
    let (mut program, (declared_0, declared_1)) =
        samara::Program::builder::<(samara::Source<ScriptedSource>, samara::Effect<Observe>)>();
    let source = declared_0;
    let observe = declared_1;
    let _component = program
        .component::<SourceProbe>(
            ComponentId::new("silent-source"),
            SourceProbeConfig {
                causal_timer: false,
                source,
                observe,
            },
        )
        .finish()
        .expect("component registration");
    let starts = Arc::new(AtomicUsize::new(0));
    let (observed, mut observations) = tokio::sync::mpsc::unbounded_channel();
    let runtime = Runtime::builder(program.build().expect("valid program"))
        .with_drivers((
            samara::DrivenSource::new(SilentSourceDriver {
                starts: starts.clone(),
            }),
            ObservationDriver {
                observations: observed,
            },
        ))
        .build()
        .expect("valid bindings");
    let task = runtime.spawn();

    assert_eq!(observations.recv().await, Some(Observation::Ended));
    let report = task.shutdown(Shutdown::Drain).await.expect("clean drain");
    assert!(report.is_clean());
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    assert!(observations.try_recv().is_err());
}

#[derive(Clone, Copy, Debug)]
enum SourcePanicMode {
    Start,
    Future,
}

struct PanickingSourceDriver {
    mode: SourcePanicMode,
    entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl SourceDriver<ScriptedSource> for PanickingSourceDriver {
    fn run(&self, _descriptor: ScriptedSource, _sink: SourceSink<ScriptedSource>) -> BoxFuture<()> {
        let entered = self.entered.lock().expect("entry lock").take();
        match self.mode {
            SourcePanicMode::Start => {
                if let Some(entered) = entered {
                    let _ = entered.send(());
                }
                panic!("SourceDriver start panic fixture");
            }
            SourcePanicMode::Future => Box::pin(async move {
                if let Some(entered) = entered {
                    let _ = entered.send(());
                }
                panic!("SourceDriver future panic fixture");
            }),
        }
    }
}

#[tokio::test]
async fn phase6_source_driver_panics_fault_and_clean_the_scope() {
    for mode in [SourcePanicMode::Start, SourcePanicMode::Future] {
        let (mut program, (declared_0, declared_1)) =
            samara::Program::builder::<(samara::Source<ScriptedSource>, samara::Effect<Observe>)>();
        let source = declared_0;
        let observe = declared_1;
        let _ = program
            .component::<SourceProbe>(
                ComponentId::new(format!("source-panic-{mode:?}")),
                SourceProbeConfig {
                    causal_timer: false,
                    source,
                    observe,
                },
            )
            .finish()
            .expect("component registration");
        let (entered, entered_rx) = tokio::sync::oneshot::channel();
        let (observed, _observations) = tokio::sync::mpsc::unbounded_channel();
        let runtime = Runtime::builder(program.build().expect("valid program"))
            .with_drivers((
                samara::DrivenSource::new(PanickingSourceDriver {
                    mode,
                    entered: Mutex::new(Some(entered)),
                }),
                ObservationDriver {
                    observations: observed,
                },
            ))
            .build()
            .expect("valid binding");
        let task = runtime.spawn();
        entered_rx.await.expect("panicking SourceDriver started");

        let error = task
            .shutdown(Shutdown::Drain)
            .await
            .expect_err("SourceDriver panic faults the live scope");
        assert_eq!(
            error.descriptor_type(),
            Some(std::any::type_name::<ScriptedSource>())
        );
    }
}

#[tokio::test]
async fn phase6_source_cutover_suppresses_late_terminal() -> Result<(), RuntimeError> {
    let SourceFixture {
        runtime,
        component,
        mut observations,
        mut sinks,
        starts,
        dropped,
    } = source_fixture(false);
    let handle = runtime.handle(&component).expect("registered Component");
    let task = runtime.spawn();
    let old = sinks.recv().await.expect("old Source started");

    handle
        .send(SourceMessage::Replace(2))
        .await
        .expect("replacement accepted");
    let new = sinks.recv().await.expect("replacement Source started");
    assert_eq!(old.end().await, Err(DriverStopped));
    new.end().await.expect("new terminal accepted");
    assert_eq!(observations.recv().await, Some(Observation::Ended));

    let report = task.shutdown(Shutdown::Drain).await.expect("clean drain");
    assert!(report.is_clean());
    assert_eq!(starts.load(Ordering::SeqCst), 2);
    assert_eq!(dropped.load(Ordering::SeqCst), 2);
    assert!(observations.try_recv().is_err());

    let SourceFixture {
        runtime,
        component,
        mut observations,
        mut sinks,
        dropped,
        ..
    } = source_fixture(false);
    let handle = runtime.handle(&component).expect("registered Component");
    let task = runtime.spawn();
    let removed = sinks.recv().await.expect("Source started");
    handle
        .send(SourceMessage::Remove)
        .await
        .expect("removal accepted");
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while dropped.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("removed Source task stops");
    assert_eq!(removed.end().await, Err(DriverStopped));
    assert!(task.shutdown(Shutdown::Drain).await?.is_clean());
    assert!(observations.try_recv().is_err());
    Ok::<(), RuntimeError>(())
}

#[tokio::test]
async fn phase6_source_cancellation_emits_no_unpromised_event() {
    let SourceFixture {
        runtime,
        mut observations,
        mut sinks,
        dropped,
        ..
    } = source_fixture(false);
    let task = runtime.spawn();
    let sink = sinks.recv().await.expect("Source started");

    let report = task.shutdown(Shutdown::Cancel).await.expect("clean cancel");
    assert!(report.is_clean());
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(sink.emit(1).await, Err(DriverStopped));
    assert_eq!(sink.end().await, Err(DriverStopped));
    assert!(observations.try_recv().is_err());
}

#[tokio::test]
async fn phase6_drain_stops_sources_and_drains_accepted_causal_work() {
    let SourceFixture {
        runtime,
        mut observations,
        mut sinks,
        dropped,
        ..
    } = source_fixture(true);
    let task = runtime.spawn();
    let sink = sinks.recv().await.expect("Source started");
    sink.emit(7).await.expect("Source delivery accepted");

    let report = task.shutdown(Shutdown::Drain).await.expect("clean drain");
    assert!(report.is_clean());
    assert_eq!(report.pending_now, 0);
    assert_eq!(report.pending_later, 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(observations.recv().await, Some(Observation::Timer(7)));
    assert!(observations.try_recv().is_err());
    assert_eq!(sink.end().await, Err(DriverStopped));
}

struct AdmissionProbe;

struct AdmissionProbeConfig {
    record: EffectCapability<Record>,
}

enum AdmissionMessage {
    Add,
    Recorded,
}

impl Component for AdmissionProbe {
    type Requires = ();
    type Config = AdmissionProbeConfig;
    type Model = u64;
    type Message = AdmissionMessage;

    fn init(
        _config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
    ) -> Init<Self::Model, Self::Message> {
        Init::new(0)
    }

    fn update(
        config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
        model: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        match message {
            AdmissionMessage::Add => {
                *model += 1;
                Command::effect_with(&config.record, Record(*model), |_| {
                    AdmissionMessage::Recorded
                })
            }
            AdmissionMessage::Recorded => Command::none(),
        }
    }
}

async fn admitted_burst(count: usize) -> Vec<u64> {
    let (mut program, (declared_0,)) = samara::Program::builder::<(samara::Effect<Record>,)>();
    let record = declared_0;
    let probe = program
        .component::<AdmissionProbe>(
            ComponentId::new("admission-probe"),
            AdmissionProbeConfig { record },
        )
        .finish()
        .expect("component registration");
    let values = Arc::new(Mutex::new(Vec::new()));
    let runtime = Runtime::builder(program.build().expect("valid program"))
        .with_drivers((RecordingDriver {
            values: values.clone(),
            observed: None,
        },))
        .build()
        .expect("valid binding");
    let handle = runtime.handle(&probe).expect("registered Component");
    let task = runtime.spawn();
    let mut sends = JoinSet::new();
    for _ in 0..count {
        let handle = handle.clone();
        sends.spawn(async move { handle.send(AdmissionMessage::Add).await });
    }
    while let Some(send) = sends.join_next().await {
        send.expect("send task").expect("admitted send");
    }
    let report = task.shutdown(Shutdown::Drain).await.expect("clean drain");
    assert!(report.is_clean());
    Arc::try_unwrap(values)
        .unwrap_or_else(|_| panic!("runtime released the recording Driver"))
        .into_inner()
        .expect("recording lock")
}

#[tokio::test]
async fn phase6_live_ingress_serializes_component_transitions() {
    let mut values = admitted_burst(128).await;
    values.sort_unstable();
    assert_eq!(values, (1..=128).collect::<Vec<_>>());
}

#[tokio::test]
async fn phase6_accepted_internal_delivery_has_no_silent_drop() {
    let count = 2_048;
    let values = admitted_burst(count).await;
    assert_eq!(values.len(), count);
    eprintln!(
        "Phase 6 unbounded-delivery characterization: accepted={count}, observed={}",
        values.len()
    );
}

#[tokio::test]
async fn phase6_shutdown_closes_external_ingress() -> Result<(), RuntimeError> {
    let (mut program, (declared_0,)) = samara::Program::builder::<(samara::Effect<Record>,)>();
    let record = declared_0;
    let probe = program
        .component::<AdmissionProbe>(
            ComponentId::new("closed-ingress"),
            AdmissionProbeConfig { record },
        )
        .finish()
        .expect("component registration");
    let values = Arc::new(Mutex::new(Vec::new()));
    let runtime = Runtime::builder(program.build().expect("valid program"))
        .with_drivers((RecordingDriver {
            values,
            observed: None,
        },))
        .build()
        .expect("valid binding");
    let handle = runtime.handle(&probe).expect("registered Component");
    let task = runtime.spawn();
    assert!(task.shutdown(Shutdown::Cancel).await?.is_clean());
    assert!(handle.send(AdmissionMessage::Add).await.is_err());
    Ok(())
}

#[derive(Debug)]
struct InitialEffect;

impl EffectDescriptor for InitialEffect {
    type Output = ();
    type Error = Infallible;
}

struct InitialEffectComponent;

struct InitialEffectComponentConfig {
    effect: EffectCapability<InitialEffect>,
}

impl Component for InitialEffectComponent {
    type Requires = ();
    type Config = InitialEffectComponentConfig;
    type Model = ();
    type Message = ();

    fn init(
        config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
    ) -> Init<Self::Model, Self::Message> {
        Init::new(()).with_command(Command::effect_with(&config.effect, InitialEffect, |_| ()))
    }

    fn update(
        _config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
        _model: &mut Self::Model,
        (): (),
    ) -> Command<Self::Message> {
        Command::none()
    }
}

struct NoopInitialDriver;

impl EffectDriver<InitialEffect> for NoopInitialDriver {
    fn execute(&self, _descriptor: InitialEffect) -> BoxFuture<Result<(), Infallible>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn phase6_knowable_live_binding_errors_fail_build() {
    let (mut program, (declared_0,)) =
        samara::Program::builder::<(samara::Effect<InitialEffect>,)>();
    let effect = declared_0;
    let _ = program
        .component::<InitialEffectComponent>(
            ComponentId::new("duplicate-effect"),
            InitialEffectComponentConfig { effect },
        )
        .finish()
        .expect("component registration");
    assert!(
        Runtime::builder(program.build().expect("valid program"))
            .with_drivers((NoopInitialDriver,))
            .bind_effect::<InitialEffect, _>(NoopInitialDriver)
            .build()
            .is_err()
    );

    let (mut program, (declared_0, declared_1)) =
        samara::Program::builder::<(samara::Source<ScriptedSource>, samara::Effect<Observe>)>();
    let source = declared_0;
    let observe = declared_1;
    let _ = program
        .component::<SourceProbe>(
            ComponentId::new("duplicate-source"),
            SourceProbeConfig {
                causal_timer: false,
                source,
                observe,
            },
        )
        .finish()
        .expect("component registration");
    let (sinks, _sink_rx) = tokio::sync::mpsc::unbounded_channel();
    let starts = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let binding = || HarnessSourceDriver {
        starts: starts.clone(),
        dropped: dropped.clone(),
        sinks: sinks.clone(),
    };
    let (observed, _observations) = tokio::sync::mpsc::unbounded_channel();
    assert!(
        Runtime::builder(program.build().expect("valid program"))
            .with_drivers((
                samara::DrivenSource::new(binding()),
                ObservationDriver {
                    observations: observed,
                },
            ))
            .bind_source::<ScriptedSource, _>(binding())
            .build()
            .is_err()
    );
}

#[derive(Debug)]
struct Query {
    fail: bool,
}

impl EffectDescriptor for Query {
    type Output = u8;
    type Error = &'static str;
}

struct QueryDriver {
    calls: Arc<AtomicUsize>,
}

impl EffectDriver<Query> for QueryDriver {
    fn execute(&self, descriptor: Query) -> BoxFuture<Result<u8, &'static str>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if descriptor.fail {
                Err("expected failure")
            } else {
                Ok(7)
            }
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum QueryObservation {
    Succeeded(u8),
    Failed(&'static str),
}

#[derive(Debug)]
struct ObserveQuery(QueryObservation);

impl EffectDescriptor for ObserveQuery {
    type Output = ();
    type Error = Infallible;
}

struct QueryObserver {
    observations: tokio::sync::mpsc::UnboundedSender<QueryObservation>,
}

impl EffectDriver<ObserveQuery> for QueryObserver {
    fn execute(&self, descriptor: ObserveQuery) -> BoxFuture<Result<(), Infallible>> {
        let observations = self.observations.clone();
        Box::pin(async move {
            let _ = observations.send(descriptor.0);
            Ok(())
        })
    }
}

enum QueryMessage {
    Start,
    Outcome(EffectOutcome<u8, &'static str>),
    Observed,
}

struct QueryProbe;

struct QueryProbeConfig {
    mapper_calls: Arc<AtomicUsize>,
    query: EffectCapability<Query>,
    observe: EffectCapability<ObserveQuery>,
}

impl Component for QueryProbe {
    type Requires = ();
    type Config = QueryProbeConfig;
    type Model = ();
    type Message = QueryMessage;

    fn init(
        _config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
    ) -> Init<Self::Model, Self::Message> {
        Init::new(())
    }

    fn update(
        config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
        _model: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        match message {
            QueryMessage::Start => {
                let success_calls = config.mapper_calls.clone();
                let failure_calls = config.mapper_calls.clone();
                Command::batch([
                    Command::effect_with(&config.query, Query { fail: false }, move |outcome| {
                        success_calls.fetch_add(1, Ordering::SeqCst);
                        QueryMessage::Outcome(outcome)
                    }),
                    Command::effect_with(&config.query, Query { fail: true }, move |outcome| {
                        failure_calls.fetch_add(1, Ordering::SeqCst);
                        QueryMessage::Outcome(outcome)
                    }),
                ])
            }
            QueryMessage::Outcome(EffectOutcome::Succeeded(value)) => Command::effect_with(
                &config.observe,
                ObserveQuery(QueryObservation::Succeeded(value)),
                |_| QueryMessage::Observed,
            ),
            QueryMessage::Outcome(EffectOutcome::Failed(error)) => Command::effect_with(
                &config.observe,
                ObserveQuery(QueryObservation::Failed(error)),
                |_| QueryMessage::Observed,
            ),
            QueryMessage::Outcome(EffectOutcome::Cancelled(_)) => {
                panic!("normal live completion is not scope cancellation")
            }
            QueryMessage::Observed => Command::none(),
        }
    }
}

#[tokio::test]
async fn phase6_effect_success_and_failure_map_exactly_once() -> Result<(), RuntimeError> {
    let driver_calls = Arc::new(AtomicUsize::new(0));
    let mapper_calls = Arc::new(AtomicUsize::new(0));
    let (observed, mut observations) = tokio::sync::mpsc::unbounded_channel();
    let (mut program, (declared_0, declared_1)) =
        samara::Program::builder::<(samara::Effect<Query>, samara::Effect<ObserveQuery>)>();
    let query = declared_0;
    let observe = declared_1;
    let probe = program
        .component::<QueryProbe>(
            ComponentId::new("query"),
            QueryProbeConfig {
                mapper_calls: mapper_calls.clone(),
                query,
                observe,
            },
        )
        .finish()
        .expect("component registration");
    let runtime = Runtime::builder(program.build().expect("valid program"))
        .with_drivers((
            QueryDriver {
                calls: driver_calls.clone(),
            },
            QueryObserver {
                observations: observed,
            },
        ))
        .build()
        .expect("valid bindings");
    let handle = runtime.handle(&probe).expect("registered Component");
    let task = runtime.spawn();
    handle.send(QueryMessage::Start).await.expect("accepted");

    let first = observations.recv().await.expect("first mapped outcome");
    let second = observations.recv().await.expect("second mapped outcome");
    assert!(
        matches!(&first, QueryObservation::Succeeded(7))
            || matches!(&second, QueryObservation::Succeeded(7))
    );
    assert!(
        matches!(&first, QueryObservation::Failed("expected failure"))
            || matches!(&second, QueryObservation::Failed("expected failure"))
    );
    assert!(task.shutdown(Shutdown::Drain).await?.is_clean());
    assert_eq!(driver_calls.load(Ordering::SeqCst), 2);
    assert_eq!(mapper_calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[derive(Debug)]
struct GateEffect;

impl EffectDescriptor for GateEffect {
    type Output = ();
    type Error = Infallible;
}

struct GateDriver {
    started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

impl EffectDriver<GateEffect> for GateDriver {
    fn execute(&self, _descriptor: GateEffect) -> BoxFuture<Result<(), Infallible>> {
        let started = self.started.lock().expect("start lock").take();
        let release = self.release.lock().expect("release lock").take();
        Box::pin(async move {
            if let Some(started) = started {
                let _ = started.send(());
            }
            if let Some(release) = release {
                let _ = release.await;
            }
            Ok(())
        })
    }
}

enum DrainGateMessage {
    Trigger,
    WantSource,
    Noop,
    Observed,
}

struct DrainGateProbe;

struct DrainGateProbeConfig {
    gate: EffectCapability<GateEffect>,
    observe: EffectCapability<Observe>,
    source: SourceCapability<ScriptedSource>,
}

impl Component for DrainGateProbe {
    type Requires = ();
    type Config = DrainGateProbeConfig;
    type Model = bool;
    type Message = DrainGateMessage;

    fn init(
        _config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
    ) -> Init<Self::Model, Self::Message> {
        Init::new(false)
    }

    fn update(
        config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
        desired: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        match message {
            DrainGateMessage::Trigger => {
                Command::effect_with(&config.gate, GateEffect, |_| DrainGateMessage::WantSource)
            }
            DrainGateMessage::WantSource => {
                *desired = true;
                Command::effect_with(&config.observe, Observe(Observation::Timer(99)), |_| {
                    DrainGateMessage::Observed
                })
            }
            DrainGateMessage::Noop | DrainGateMessage::Observed => Command::none(),
        }
    }

    fn subscriptions(
        config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
        desired: &Self::Model,
    ) -> Subscriptions<Self::Message> {
        if *desired {
            Subscriptions::one(Subscription::source_with(
                &config.source,
                SubscriptionId::new("late-source"),
                ScriptedSource { generation: 1 },
                |_| DrainGateMessage::Noop,
            ))
        } else {
            Subscriptions::none()
        }
    }
}

#[tokio::test]
async fn phase6_drain_realizes_no_new_sources() {
    let (started, started_rx) = tokio::sync::oneshot::channel();
    let (release, release_rx) = tokio::sync::oneshot::channel();
    let (sink_tx, _sinks) = tokio::sync::mpsc::unbounded_channel();
    let starts = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let (observed, mut observations) = tokio::sync::mpsc::unbounded_channel();
    let (mut program, (declared_0, declared_1, declared_2)) = samara::Program::builder::<(
        samara::Effect<GateEffect>,
        samara::Effect<Observe>,
        samara::Source<ScriptedSource>,
    )>();
    let gate = declared_0;
    let observe = declared_1;
    let source = declared_2;
    let probe = program
        .component::<DrainGateProbe>(
            ComponentId::new("drain-gate"),
            DrainGateProbeConfig {
                gate,
                observe,
                source,
            },
        )
        .finish()
        .expect("component registration");
    let runtime = Runtime::builder(program.build().expect("valid program"))
        .with_drivers((
            GateDriver {
                started: Mutex::new(Some(started)),
                release: Mutex::new(Some(release_rx)),
            },
            ObservationDriver {
                observations: observed,
            },
            samara::DrivenSource::new(HarnessSourceDriver {
                starts: starts.clone(),
                dropped,
                sinks: sink_tx,
            }),
        ))
        .build()
        .expect("valid bindings");
    let handle = runtime.handle(&probe).expect("registered Component");
    let task = runtime.spawn();
    handle
        .send(DrainGateMessage::Trigger)
        .await
        .expect("accepted");
    started_rx.await.expect("finite effect started");

    let shutdown = tokio::spawn(task.shutdown(Shutdown::Drain));
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if handle.send(DrainGateMessage::Noop).await.is_err() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("Drain closes ingress");
    release.send(()).expect("effect still owned");
    let report = shutdown.await.expect("shutdown task").expect("clean drain");
    assert!(report.is_clean());
    assert_eq!(observations.recv().await, Some(Observation::Timer(99)));
    assert_eq!(starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn phase6_successful_shutdown_owns_zero_work() -> Result<(), RuntimeError> {
    for mode in [Shutdown::Drain, Shutdown::Cancel] {
        let (mut program, (declared_0,)) = samara::Program::builder::<(samara::Effect<Record>,)>();
        let record = declared_0;
        let _ = program
            .component::<AdmissionProbe>(
                ComponentId::new(format!("empty-{mode:?}")),
                AdmissionProbeConfig { record },
            )
            .finish()
            .expect("component registration");
        let runtime = Runtime::builder(program.build().expect("valid program"))
            .with_drivers((RecordingDriver {
                values: Arc::new(Mutex::new(Vec::new())),
                observed: None,
            },))
            .build()
            .expect("valid binding");
        let report = runtime.spawn().shutdown(mode).await?;
        assert_eq!(report.remaining, 0);
        assert_eq!(report.pending_now, 0);
        assert_eq!(report.pending_later, 0);
    }
    Ok(())
}

/// The echo Port's vocabulary; the request carries its own reply token.
enum EchoMessage {
    Echo(u64, ReplyWith<u64>),
}

enum EchoProviderMessage {
    Echo(EchoMessage),
    Seen,
}

#[derive(Debug)]
struct ProviderSeen;

impl EffectDescriptor for ProviderSeen {
    type Output = ();
    type Error = Infallible;
}

struct ProviderSeenDriver {
    seen: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl EffectDriver<ProviderSeen> for ProviderSeenDriver {
    fn execute(&self, _descriptor: ProviderSeen) -> BoxFuture<Result<(), Infallible>> {
        let seen = self.seen.lock().expect("seen lock").take();
        Box::pin(async move {
            if let Some(seen) = seen {
                let _ = seen.send(());
            }
            Ok(())
        })
    }
}

struct EchoProvider;

struct EchoProviderConfig {
    reply: bool,
    seen: EffectCapability<ProviderSeen>,
}

impl Component for EchoProvider {
    type Requires = ();
    type Config = EchoProviderConfig;
    type Model = ();
    type Message = EchoProviderMessage;

    fn init(
        _config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
    ) -> Init<Self::Model, Self::Message> {
        Init::new(())
    }

    fn update(
        config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
        _model: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        match message {
            EchoProviderMessage::Echo(EchoMessage::Echo(value, reply_with)) => {
                if config.reply {
                    Command::reply(reply_with, value)
                } else {
                    Command::effect_with(&config.seen, ProviderSeen, |_| EchoProviderMessage::Seen)
                }
            }
            EchoProviderMessage::Seen => Command::none(),
        }
    }
}

#[derive(Debug)]
struct ObserveReply(u64);

impl EffectDescriptor for ObserveReply {
    type Output = ();
    type Error = Infallible;
}

struct ReplyObserver {
    replies: tokio::sync::mpsc::UnboundedSender<u64>,
}

impl EffectDriver<ObserveReply> for ReplyObserver {
    fn execute(&self, descriptor: ObserveReply) -> BoxFuture<Result<(), Infallible>> {
        let replies = self.replies.clone();
        Box::pin(async move {
            let _ = replies.send(descriptor.0);
            Ok(())
        })
    }
}

enum EchoRequesterMessage {
    Start,
    Outcome(RequestOutcome<u64>),
    Observed,
}

struct EchoRequester;

struct EchoRequesterConfig {
    echo: Port<EchoMessage>,
    continuation_calls: Arc<AtomicUsize>,
    observe: EffectCapability<ObserveReply>,
}

impl Component for EchoRequester {
    type Requires = ();
    type Config = EchoRequesterConfig;
    type Model = ();
    type Message = EchoRequesterMessage;

    fn init(
        _config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
    ) -> Init<Self::Model, Self::Message> {
        Init::new(())
    }

    fn update(
        config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
        _model: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        match message {
            EchoRequesterMessage::Start => {
                let continuation_calls = config.continuation_calls.clone();
                Command::request_to(
                    &config.echo,
                    |reply_with| EchoMessage::Echo(42, reply_with),
                    move |outcome| {
                        continuation_calls.fetch_add(1, Ordering::SeqCst);
                        EchoRequesterMessage::Outcome(outcome)
                    },
                )
            }
            EchoRequesterMessage::Outcome(RequestOutcome::Replied(value)) => {
                Command::effect_with(&config.observe, ObserveReply(value), |_| {
                    EchoRequesterMessage::Observed
                })
            }
            EchoRequesterMessage::Outcome(_) => {
                panic!("Phase 6 does not manufacture deferred Request outcomes")
            }
            EchoRequesterMessage::Observed => Command::none(),
        }
    }
}

fn echo_program(
    reply: bool,
    continuation_calls: Arc<AtomicUsize>,
) -> (TestProgram2, ComponentRef<EchoRequester>) {
    let (mut program, (declared_0, declared_1)) =
        samara::Program::builder::<(samara::Effect<ProviderSeen>, samara::Effect<ObserveReply>)>();
    let echo = program.port::<EchoMessage>(PortId::new("echo"));
    let seen = declared_0;
    let observe = declared_1;
    let provider = program
        .component::<EchoProvider>(
            ComponentId::new("echo-provider"),
            EchoProviderConfig { reply, seen },
        )
        .finish()
        .expect("component registration");
    program.connect(&echo, &provider.accepting(EchoProviderMessage::Echo));
    let requester = program
        .component::<EchoRequester>(
            ComponentId::new("echo-requester"),
            EchoRequesterConfig {
                echo,
                continuation_calls,
                observe,
            },
        )
        .finish()
        .expect("component registration");
    (program.build().expect("valid Echo program"), requester)
}

#[tokio::test]
async fn phase6_live_request_reply_drains_causally() -> Result<(), RuntimeError> {
    let continuation_calls = Arc::new(AtomicUsize::new(0));
    let (program, requester) = echo_program(true, continuation_calls.clone());
    let (replies, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let (seen, _seen_rx) = tokio::sync::oneshot::channel();
    let runtime = Runtime::builder(program)
        .with_drivers((
            ProviderSeenDriver {
                seen: Mutex::new(Some(seen)),
            },
            ReplyObserver { replies },
        ))
        .build()?;
    let handle = runtime.handle(&requester)?;
    let task = runtime.spawn();
    handle.send(EchoRequesterMessage::Start).await?;
    let report = task.shutdown(Shutdown::Drain).await?;
    assert!(report.is_clean());
    assert_eq!(observed.recv().await, Some(42));
    assert_eq!(continuation_calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn phase6_cancel_drops_unanswered_request_without_mapping() -> Result<(), RuntimeError> {
    let continuation_calls = Arc::new(AtomicUsize::new(0));
    let (program, requester) = echo_program(false, continuation_calls.clone());
    let (seen, seen_rx) = tokio::sync::oneshot::channel();
    let (replies, _observed) = tokio::sync::mpsc::unbounded_channel();
    let runtime = Runtime::builder(program)
        .with_drivers((
            ProviderSeenDriver {
                seen: Mutex::new(Some(seen)),
            },
            ReplyObserver { replies },
        ))
        .build()?;
    let handle = runtime.handle(&requester)?;
    let task = runtime.spawn();
    handle.send(EchoRequesterMessage::Start).await?;
    seen_rx.await.expect("provider accepted the Request");
    let report = task.shutdown(Shutdown::Cancel).await?;
    assert!(report.is_clean());
    assert_eq!(continuation_calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn shutdown_escalation_closes_unanswered_request_without_mapping() -> Result<(), RuntimeError>
{
    let continuation_calls = Arc::new(AtomicUsize::new(0));
    let (program, requester) = echo_program(false, continuation_calls.clone());
    let (seen, seen_rx) = tokio::sync::oneshot::channel();
    let (replies, _observed) = tokio::sync::mpsc::unbounded_channel();
    let runtime = Runtime::builder(program)
        .with_drivers((
            ProviderSeenDriver {
                seen: Mutex::new(Some(seen)),
            },
            ReplyObserver { replies },
        ))
        .build()?;
    let handle = runtime.handle(&requester)?;
    let mut task = runtime.spawn();
    handle.send(EchoRequesterMessage::Start).await?;
    seen_rx.await.expect("provider accepted the Request");

    task.request_shutdown(Shutdown::Drain);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(5), task.run_forever())
            .await
            .is_err()
    );
    task.request_shutdown(Shutdown::Cancel);
    let report = tokio::time::timeout(std::time::Duration::from_secs(1), task.run_forever())
        .await
        .expect("Cancel closes the unanswered Request")?;
    assert_eq!(report.remaining, 0);
    assert_eq!(report.pending_now, 0);
    assert_eq!(report.pending_later, 0);
    assert_eq!(continuation_calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[derive(Debug)]
struct Independent(char);

impl EffectDescriptor for Independent {
    type Output = char;
    type Error = Infallible;
}

struct IndependentDriver {
    started: tokio::sync::mpsc::UnboundedSender<char>,
    releases: Mutex<std::collections::HashMap<char, tokio::sync::oneshot::Receiver<()>>>,
}

impl EffectDriver<Independent> for IndependentDriver {
    fn execute(&self, descriptor: Independent) -> BoxFuture<Result<char, Infallible>> {
        let label = descriptor.0;
        let release = self
            .releases
            .lock()
            .expect("release map")
            .remove(&label)
            .expect("one release per independent effect");
        let started = self.started.clone();
        Box::pin(async move {
            let _ = started.send(label);
            let _ = release.await;
            Ok(label)
        })
    }
}

#[derive(Debug)]
struct RecordOrder(char);

impl EffectDescriptor for RecordOrder {
    type Output = ();
    type Error = Infallible;
}

struct OrderObserver {
    order: tokio::sync::mpsc::UnboundedSender<char>,
}

impl EffectDriver<RecordOrder> for OrderObserver {
    fn execute(&self, descriptor: RecordOrder) -> BoxFuture<Result<(), Infallible>> {
        let order = self.order.clone();
        Box::pin(async move {
            let _ = order.send(descriptor.0);
            Ok(())
        })
    }
}

enum IndependentMessage {
    Start,
    Completed(EffectOutcome<char, Infallible>),
    Recorded,
}

struct IndependentProbe;

struct IndependentProbeConfig {
    independent: EffectCapability<Independent>,
    record: EffectCapability<RecordOrder>,
}

impl Component for IndependentProbe {
    type Requires = ();
    type Config = IndependentProbeConfig;
    type Model = ();
    type Message = IndependentMessage;

    fn init(
        _config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
    ) -> Init<Self::Model, Self::Message> {
        Init::new(())
    }

    fn update(
        config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
        _model: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        match message {
            IndependentMessage::Start => Command::batch([
                Command::effect_with(
                    &config.independent,
                    Independent('A'),
                    IndependentMessage::Completed,
                ),
                Command::effect_with(
                    &config.independent,
                    Independent('B'),
                    IndependentMessage::Completed,
                ),
            ]),
            IndependentMessage::Completed(EffectOutcome::Succeeded(label)) => {
                Command::effect_with(&config.record, RecordOrder(label), |_| {
                    IndependentMessage::Recorded
                })
            }
            IndependentMessage::Completed(EffectOutcome::Failed(never)) => match never {},
            IndependentMessage::Completed(EffectOutcome::Cancelled(_)) => {
                panic!("the gated effects complete normally")
            }
            IndependentMessage::Recorded => Command::none(),
        }
    }
}

async fn forced_independent_order(first: char, second: char) -> Vec<char> {
    let (release_a, wait_a) = tokio::sync::oneshot::channel();
    let (release_b, wait_b) = tokio::sync::oneshot::channel();
    let releases = std::collections::HashMap::from([('A', wait_a), ('B', wait_b)]);
    let mut release = std::collections::HashMap::from([('A', release_a), ('B', release_b)]);
    let (started, mut starts) = tokio::sync::mpsc::unbounded_channel();
    let (order, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let (mut program, (declared_0, declared_1)) =
        samara::Program::builder::<(samara::Effect<Independent>, samara::Effect<RecordOrder>)>();
    let independent = declared_0;
    let record = declared_1;
    let probe = program
        .component::<IndependentProbe>(
            ComponentId::new("independent"),
            IndependentProbeConfig {
                independent,
                record,
            },
        )
        .finish()
        .expect("component registration");
    let runtime = Runtime::builder(program.build().expect("valid program"))
        .with_drivers((
            IndependentDriver {
                started,
                releases: Mutex::new(releases),
            },
            OrderObserver { order },
        ))
        .build()
        .expect("valid bindings");
    let handle = runtime.handle(&probe).expect("registered Component");
    let task = runtime.spawn();
    handle
        .send(IndependentMessage::Start)
        .await
        .expect("accepted");
    let mut started_labels = vec![
        starts.recv().await.expect("first Driver started"),
        starts.recv().await.expect("second Driver started"),
    ];
    started_labels.sort_unstable();
    assert_eq!(started_labels, vec!['A', 'B']);

    release
        .remove(&first)
        .expect("first release")
        .send(())
        .expect("first Driver pending");
    assert_eq!(observed.recv().await, Some(first));
    release
        .remove(&second)
        .expect("second release")
        .send(())
        .expect("second Driver pending");
    assert_eq!(observed.recv().await, Some(second));
    assert!(
        task.shutdown(Shutdown::Drain)
            .await
            .expect("clean drain")
            .is_clean()
    );
    vec![first, second]
}

#[tokio::test]
async fn v8_independent_live_events_accept_either_order() {
    assert_eq!(forced_independent_order('A', 'B').await, vec!['A', 'B']);
    assert_eq!(forced_independent_order('B', 'A').await, vec!['B', 'A']);
}

fn assert_same_causal_partial_order(order: &[char]) {
    assert_eq!(order.len(), 2);
    assert!(order.contains(&'A'));
    assert!(order.contains(&'B'));
}

#[tokio::test]
async fn v8_conformance_compares_partial_order_not_scheduler_sequence() {
    let a_then_b = forced_independent_order('A', 'B').await;
    let b_then_a = forced_independent_order('B', 'A').await;
    assert_ne!(a_then_b, b_then_a);
    assert_same_causal_partial_order(&a_then_b);
    assert_same_causal_partial_order(&b_then_a);
}

fn query_program(mapper_calls: Arc<AtomicUsize>) -> (TestProgram3, ComponentRef<QueryProbe>) {
    let (mut program, (declared_0, declared_1)) =
        samara::Program::builder::<(samara::Effect<Query>, samara::Effect<ObserveQuery>)>();
    let query = declared_0;
    let observe = declared_1;
    let probe = program
        .component::<QueryProbe>(
            ComponentId::new("query-parity"),
            QueryProbeConfig {
                mapper_calls,
                query,
                observe,
            },
        )
        .finish()
        .expect("component registration");
    (program.build().expect("valid query program"), probe)
}

fn normalize_query_observations(observations: Vec<QueryObservation>) -> Vec<String> {
    let mut normalized = observations
        .into_iter()
        .map(|observation| match observation {
            QueryObservation::Succeeded(value) => format!("succeeded:{value}"),
            QueryObservation::Failed(error) => format!("failed:{error}"),
        })
        .collect::<Vec<_>>();
    normalized.sort();
    normalized
}

#[tokio::test]
async fn v5_live_and_controlled_boundaries_map_equivalent_observations() -> Result<(), RuntimeError>
{
    let (program, probe) = query_program(Arc::new(AtomicUsize::new(0)));
    let mut controlled = ControlledRuntime::builder(program).control_all().build()?;
    controlled.send(&probe, QueryMessage::Start)?;
    controlled.run_until_idle()?;
    for _ in 0..2 {
        let pending = controlled.next_effect::<Query>()?;
        let fail = pending.intent.fail;
        if fail {
            controlled.complete(pending, EffectOutcome::Failed("expected failure"))?;
        } else {
            controlled.complete(pending, EffectOutcome::Succeeded(7))?;
        }
    }
    controlled.run_until_idle()?;
    let mut controlled_observations = Vec::new();
    for _ in 0..2 {
        let pending = controlled.next_effect::<ObserveQuery>()?;
        controlled_observations.push(pending.intent.0.clone());
        controlled.complete(pending, EffectOutcome::Succeeded(()))?;
    }
    controlled.run_until_idle()?;
    assert!(controlled.cancel()?.is_clean());

    let (program, probe) = query_program(Arc::new(AtomicUsize::new(0)));
    let (observed, mut observations) = tokio::sync::mpsc::unbounded_channel();
    let runtime = Runtime::builder(program)
        .with_drivers((
            QueryDriver {
                calls: Arc::new(AtomicUsize::new(0)),
            },
            QueryObserver {
                observations: observed,
            },
        ))
        .build()?;
    let handle = runtime.handle(&probe)?;
    let task = runtime.spawn();
    handle.send(QueryMessage::Start).await?;
    let live_observations = vec![
        observations.recv().await.expect("first live observation"),
        observations.recv().await.expect("second live observation"),
    ];
    assert!(task.shutdown(Shutdown::Drain).await?.is_clean());

    assert_eq!(
        normalize_query_observations(controlled_observations),
        normalize_query_observations(live_observations)
    );
    Ok(())
}
