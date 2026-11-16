use std::convert::Infallible;

use samara::prelude::*;

#[derive(Debug)]
struct DynamicallyReachedEffect;

impl EffectDescriptor for DynamicallyReachedEffect {
    type Output = ();
    type Error = Infallible;
}

enum FaultMessage {
    Trigger,
}

struct FaultProbe;

struct FaultProbeConfig {
    effect: Effect<DynamicallyReachedEffect>,
}

impl Component for FaultProbe {
    type Io = ();
    type Connections = ();
    type Configuration = FaultProbeConfig;
    type Model = ();
    type Message = FaultMessage;

    fn init(_: &Context<Self>) -> Init<Self::Model, Self::Message> {
        Init::new(())
    }

    fn update(
        cx: &Context<Self>,
        (): &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        match message {
            FaultMessage::Trigger => {
                Effect::perform(&cx.config.effect, DynamicallyReachedEffect).discard_outcome()
            }
        }
    }
}

#[tokio::test]
async fn live_host_observes_a_later_foreign_effect_fault() {
    let effect = <Effect<DynamicallyReachedEffect>>::detached();
    let mut program = Program::builder();
    // Deliberately nonconforming Config exercises the runtime provenance defense.
    let probe = program
        .configure_component::<FaultProbe>()
        .name("fault")
        .config(FaultProbeConfig { effect })
        .add(());
    let runtime = Runtime::builder(program.build().unwrap()).build().unwrap();
    let handle = runtime.handle(&probe).unwrap();
    let mut task = runtime.spawn();
    handle.send(FaultMessage::Trigger).await.unwrap();
    let error = task.run_forever().await.unwrap_err();
    assert!(error.to_string().contains("another Program"));
}

struct Idle;

impl Component for Idle {
    type Io = ();
    type Connections = ();
    type Configuration = ();
    type Model = usize;
    type Message = ();

    fn init(_: &Context<Self>) -> Init<Self::Model, Self::Message> {
        Init::new(0)
    }

    fn update(
        _: &Context<Self>,
        count: &mut Self::Model,
        (): Self::Message,
    ) -> Command<Self::Message> {
        *count += 1;
        Command::none()
    }
}

#[derive(Debug)]
struct HostShutdownError;

async fn host_shutdown() -> Result<(), HostShutdownError> {
    // Ensure `run_forever` is first polled and left pending before the host
    // branch wins. Cancelling observation must leave the owner in RuntimeTask.
    tokio::task::yield_now().await;
    Ok(())
}

#[tokio::test]
async fn cancelled_run_forever_observation_preserves_owner_for_explicit_shutdown() {
    for mode in [Shutdown::Drain, Shutdown::Cancel] {
        let mut program = Program::builder();
        let idle = program
            .configure_component::<Idle>()
            .name("host-lifecycle-idle")
            .add(());
        let runtime = Runtime::builder(program.build().expect("valid program"))
            .build()
            .expect("valid live runtime");
        let handle = runtime.handle(&idle).expect("live handle");
        let mut task = runtime.spawn();

        tokio::select! {
            biased;
            result = task.run_forever() => {
                panic!("healthy runtime ended before the host shutdown future: {result:?}");
            }
            signal = host_shutdown() => {
                signal.expect("host shutdown errors remain visible to the host");
            }
        }

        handle
            .send(())
            .await
            .expect("cancelling observation must not close runtime ingress");
        let report = task
            .shutdown(mode)
            .await
            .expect("the explicitly selected shutdown policy succeeds");
        assert!(report.is_clean());
    }
}

#[tokio::test]
async fn shutdown_requests_close_ingress_and_preserve_completed_result() {
    for mode in [Shutdown::Drain, Shutdown::Cancel] {
        let mut program = Program::builder();
        let idle = program
            .configure_component::<Idle>()
            .name("shutdown-requests")
            .add(());
        let runtime = Runtime::builder(program.build().expect("valid program"))
            .build()
            .expect("valid live runtime");
        let handle = runtime.handle(&idle).expect("live handle");
        let mut task = runtime.spawn();

        task.request_shutdown(mode);
        assert!(
            handle.send(()).await.is_err(),
            "ingress closes synchronously"
        );
        task.request_shutdown(mode);
        let report = task.run_forever().await.expect("clean shutdown");
        assert!(report.is_clean());
        assert_eq!(report.pending_now, 0);
        assert_eq!(report.pending_later, 0);

        for repeated in [Shutdown::Cancel, Shutdown::Drain] {
            task.request_shutdown(repeated);
            assert_eq!(task.run_forever().await, Ok(report.clone()));
        }
        assert_eq!(task.shutdown(Shutdown::Drain).await, Ok(report));
    }
}

struct RecurringTimer;

impl Component for RecurringTimer {
    type Io = ();
    type Connections = ();
    type Configuration = ();
    type Model = ();
    type Message = ();

    fn init(_: &Context<Self>) -> Init<(), ()> {
        Init::new(()).with_command(Command::after(std::time::Duration::ZERO, ()))
    }

    fn update(_: &Context<Self>, (): &mut (), (): ()) -> Command<()> {
        Command::after(std::time::Duration::from_millis(1), ())
    }
}

#[tokio::test]
async fn shutdown_grace_period_escalates_recurring_timer_and_joins() {
    let mut program = Program::builder();
    program
        .configure_component::<RecurringTimer>()
        .name("recurring-timer")
        .add(());
    let mut task = Runtime::builder(program.build().expect("valid program"))
        .build()
        .expect("valid runtime")
        .spawn();

    // The ADR's host composition: cancelling observation at grace expiry
    // retains ownership, so escalation can still await structured cleanup.
    task.request_shutdown(Shutdown::Drain);
    let result =
        match tokio::time::timeout(std::time::Duration::from_millis(5), task.run_forever()).await {
            Ok(result) => panic!("recurring work must keep Drain pending: {result:?}"),
            Err(_) => {
                task.request_shutdown(Shutdown::Cancel);
                tokio::time::timeout(std::time::Duration::from_secs(1), task.run_forever())
                    .await
                    .expect("Cancel joins the recurring timer")
            }
        };
    let report = result.expect("clean cancellation");
    assert_eq!(report.remaining, 0);
    assert_eq!(report.pending_now, 0);
    assert_eq!(report.pending_later, 0);
}

/// Counts how many Components were told to stop.
struct Stopping;
impl Component for Stopping {
    type Io = ();
    type Connections = ();
    type Configuration = std::sync::Arc<std::sync::atomic::AtomicUsize>;
    type Model = ();
    type Message = ();

    fn init(_: &Context<Self>) -> Init<(), ()> {
        Init::new(())
    }

    fn update(_: &Context<Self>, (): &mut (), (): ()) -> Command<()> {
        Command::none()
    }

    fn shutdown(cx: &Context<Self>, (): ()) -> Command<NoMessage> {
        cx.config.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Command::none()
    }
}

#[tokio::test]
async fn run_until_drains_when_the_signal_completes() {
    let stopped = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut program = Program::builder();
    program
        .configure_component::<Stopping>()
        .config(stopped.clone())
        .add(());
    let runtime = Runtime::builder(program.build().unwrap()).build().unwrap();

    let (signal, wait) = tokio::sync::oneshot::channel::<()>();
    let run = tokio::spawn(runtime.run_until(wait));
    tokio::task::yield_now().await;
    assert!(!run.is_finished(), "a healthy program runs until signalled");
    assert_eq!(stopped.load(std::sync::atomic::Ordering::SeqCst), 0);

    signal.send(()).unwrap();
    let report = run.await.unwrap().expect("a clean drain");
    assert!(report.is_clean());
    assert_eq!(
        stopped.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "Drain calls Component::shutdown"
    );
}

#[tokio::test]
async fn run_until_returns_a_fault_without_waiting_for_the_signal() {
    let mut program = Program::builder();
    let probe = program
        .configure_component::<FaultProbe>()
        .config(FaultProbeConfig {
            effect: <Effect<DynamicallyReachedEffect>>::detached(),
        })
        .add(());
    let runtime = Runtime::builder(program.build().unwrap()).build().unwrap();
    let handle = runtime.handle(&probe).unwrap();

    let run = tokio::spawn(runtime.run_until(std::future::pending::<()>()));
    handle.send(FaultMessage::Trigger).await.unwrap();
    let error = tokio::time::timeout(std::time::Duration::from_secs(2), run)
        .await
        .expect("a fault ends run_until even though the signal never fires")
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("another Program"));
}
