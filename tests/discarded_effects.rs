use std::convert::Infallible;
use std::future::pending;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use samara::prelude::*;

#[test]
fn print_macros_preserve_familiar_formatting_and_select_the_stream() {
    let mut program = Program::builder();
    let stdout = program.effect::<PrintStdout>();
    let stderr = program.effect::<PrintStderr>();
    let subject = String::from("Samara");
    let print: Command<()> = samara::print!(&stdout, "hello {subject} {}", 1 + 1);
    let println: Command<()> = samara::println!(&stdout);
    let eprint: Command<()> = samara::eprint!(&stderr, "error: {code:04}", code = 7);
    let eprintln: Command<()> = samara::eprintln!(&stderr, "{subject}\n");

    assert_eq!(
        print.effect_intent::<PrintStdout>().unwrap().as_str(),
        "hello Samara 2"
    );
    assert_eq!(
        println.effect_intent::<PrintStdout>().unwrap().as_str(),
        "\n"
    );
    assert_eq!(
        eprint.effect_intent::<PrintStderr>().unwrap().as_str(),
        "error: 0007"
    );
    assert_eq!(
        eprintln.effect_intent::<PrintStderr>().unwrap().as_str(),
        "Samara\n\n"
    );
}

#[derive(Debug, Default, PartialEq, Eq)]
struct PrintModel {
    transitions: usize,
}

enum PrintMessage {
    Start,
}

struct PrintProbe {
    stdout: EffectCapability<PrintStdout>,
}

impl Component for PrintProbe {
    type Model = PrintModel;
    type Message = PrintMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(PrintModel::default())
    }

    fn update(
        &self,
        model: &mut Self::Model,
        PrintMessage::Start: Self::Message,
    ) -> Command<Self::Message> {
        model.transitions += 1;
        samara::println!(&self.stdout, "transition {}", model.transitions)
    }
}

#[test]
fn controlled_discarded_outcome_is_traced_without_scheduling_a_message() {
    let mut program = Program::builder();
    let stdout = program.effect::<PrintStdout>();
    let probe = program.component(ComponentId::new("discarded-print"), PrintProbe { stdout });
    let mut runtime = ControlledRuntime::builder(program.build().unwrap())
        .control_effect::<PrintStdout>()
        .build()
        .unwrap();

    runtime.send(&probe, PrintMessage::Start).unwrap();
    assert_eq!(runtime.run_until_idle().unwrap().transitions, 1);
    assert_eq!(runtime.pending_work().pending_later, 1);

    let pending = runtime.next_effect::<PrintStdout>().unwrap();
    assert_eq!(pending.intent.as_str(), "transition 1\n");
    runtime
        .complete(pending, EffectOutcome::Succeeded(()))
        .unwrap();

    assert_eq!(runtime.run_until_idle().unwrap().transitions, 0);
    assert_eq!(runtime.state(&probe).unwrap().transitions, 1);
    assert_eq!(runtime.pending_work(), PendingWork::default());

    let outcome = runtime
        .trace()
        .iter()
        .find(|record| matches!(record.event, TraceEvent::EffectOutcome { .. }))
        .expect("discarded outcome remains structurally observable");
    assert!(!runtime.trace().iter().any(|record| {
        record.cause == Some(outcome.id) && matches!(record.event, TraceEvent::Transition { .. })
    }));
    assert!(runtime.cancel().unwrap().is_clean());
}

#[test]
fn controlled_discarded_cancellation_is_traced_without_scheduling_a_message() {
    let (program, probe) = gated_program();
    let mut runtime = ControlledRuntime::builder(program)
        .control_effect::<GateEffect>()
        .build()
        .unwrap();

    runtime.send(&probe, GateMessage::Start).unwrap();
    runtime.run_until_idle().unwrap();
    let pending = runtime.next_effect::<GateEffect>().unwrap();
    runtime
        .complete(pending, EffectOutcome::Cancelled(CancelReason::Superseded))
        .unwrap();

    assert_eq!(runtime.run_until_idle().unwrap().transitions, 0);
    assert_eq!(runtime.pending_work(), PendingWork::default());
    assert_eq!(
        runtime
            .trace()
            .iter()
            .filter(|record| matches!(
                record.event,
                TraceEvent::EffectOutcome {
                    outcome: EffectOutcomeKind::Cancelled,
                    ..
                }
            ))
            .count(),
        1
    );
    assert!(runtime.cancel().unwrap().is_clean());
}

#[test]
fn discarded_outcome_dependency_is_validated_during_controlled_build() {
    let (program, _probe) = gated_program();
    let error = ControlledRuntime::builder(program)
        .build()
        .err()
        .expect("the declared Effect must be controlled before execution");
    assert!(error.to_string().contains("GateEffect"));
}

#[derive(Debug)]
struct GateEffect;

impl EffectDescriptor for GateEffect {
    type Output = u8;
    type Error = Infallible;
}

enum GateMessage {
    Start,
    Noop,
}

struct GateProbe {
    effect: EffectCapability<GateEffect>,
}

impl Component for GateProbe {
    type Model = ();
    type Message = GateMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(())
    }

    fn update(&self, _model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            GateMessage::Start => Command::effect_discarding_outcome(&self.effect, GateEffect),
            GateMessage::Noop => Command::none(),
        }
    }
}

struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct GatedDriver {
    entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    dropped: Arc<AtomicBool>,
}

impl EffectDriver<GateEffect> for GatedDriver {
    fn execute(&self, _descriptor: GateEffect) -> BoxFuture<Result<u8, Infallible>> {
        let entered = self.entered.lock().unwrap().take();
        let release = self.release.lock().unwrap().take();
        let dropped = self.dropped.clone();
        Box::pin(async move {
            let _drop = DropFlag(dropped);
            if let Some(entered) = entered {
                let _ = entered.send(());
            }
            if let Some(release) = release {
                let _ = release.await;
            } else {
                pending().await
            }
            Ok(7)
        })
    }
}

fn gated_program() -> (Program, ComponentRef<GateProbe>) {
    let mut program = Program::builder();
    let effect = program.effect::<GateEffect>();
    let probe = program.component(ComponentId::new("discarded-gate"), GateProbe { effect });
    (program.build().unwrap(), probe)
}

#[tokio::test]
async fn live_drain_waits_for_a_discarded_outcome_effect() {
    let (program, probe) = gated_program();
    let (entered, entered_rx) = tokio::sync::oneshot::channel();
    let (release, release_rx) = tokio::sync::oneshot::channel();
    let dropped = Arc::new(AtomicBool::new(false));
    let runtime = LiveRuntime::builder(program)
        .bind_effect::<GateEffect, _>(GatedDriver {
            entered: Mutex::new(Some(entered)),
            release: Mutex::new(Some(release_rx)),
            dropped: dropped.clone(),
        })
        .build()
        .unwrap();
    let handle = runtime.handle(&probe).unwrap();
    let task = runtime.spawn();

    handle.send(GateMessage::Start).await.unwrap();
    entered_rx.await.unwrap();
    let shutdown = tokio::spawn(task.shutdown(Shutdown::Drain));
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if handle.send(GateMessage::Noop).await.is_err() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("Drain closes ingress");
    assert!(
        !shutdown.is_finished(),
        "Drain must retain the finite effect obligation"
    );
    release.send(()).unwrap();

    let report = tokio::time::timeout(Duration::from_secs(1), shutdown)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(report.is_clean());
    assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn live_cancel_aborts_a_discarded_outcome_effect() {
    let (program, probe) = gated_program();
    let (entered, entered_rx) = tokio::sync::oneshot::channel();
    let dropped = Arc::new(AtomicBool::new(false));
    let runtime = LiveRuntime::builder(program)
        .bind_effect::<GateEffect, _>(GatedDriver {
            entered: Mutex::new(Some(entered)),
            release: Mutex::new(None),
            dropped: dropped.clone(),
        })
        .build()
        .unwrap();
    let handle = runtime.handle(&probe).unwrap();
    let task = runtime.spawn();

    handle.send(GateMessage::Start).await.unwrap();
    entered_rx.await.unwrap();
    let report = task.shutdown(Shutdown::Cancel).await.unwrap();

    assert!(report.is_clean());
    assert!(dropped.load(Ordering::SeqCst));
}
