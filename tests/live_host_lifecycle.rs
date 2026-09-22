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

struct FaultProbe {
    effect: EffectCapability<DynamicallyReachedEffect>,
}

impl Component for FaultProbe {
    type Model = ();
    type Message = FaultMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(())
    }

    fn update(&self, (): &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            FaultMessage::Trigger => {
                Command::effect_discarding_outcome(&self.effect, DynamicallyReachedEffect)
            }
        }
    }
}

#[test]
fn live_build_rejects_later_reached_effect_without_driver() {
    let mut program = Program::builder();
    let effect = program.effect::<DynamicallyReachedEffect>();
    let _probe = program.component(
        ComponentId::new("host-lifecycle-fault"),
        FaultProbe { effect },
    );
    let _trigger = FaultMessage::Trigger;
    let error = LiveRuntime::builder(program.build().expect("valid program"))
        .build()
        .err()
        .expect("declared effect dependencies must be bound before spawn");

    assert!(
        error
            .to_string()
            .contains(std::any::type_name::<DynamicallyReachedEffect>())
    );
}

struct Idle;

impl Component for Idle {
    type Model = usize;
    type Message = ();

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(0)
    }

    fn update(&self, count: &mut Self::Model, (): Self::Message) -> Command<Self::Message> {
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
        let idle = program.component(ComponentId::new("host-lifecycle-idle"), Idle);
        let runtime = LiveRuntime::builder(program.build().expect("valid program"))
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
        let idle = program.component(ComponentId::new("shutdown-requests"), Idle);
        let runtime = LiveRuntime::builder(program.build().expect("valid program"))
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
            assert_eq!(task.run_forever().await, Ok(report));
        }
        assert_eq!(task.shutdown(Shutdown::Drain).await, Ok(report));
    }
}

struct RecurringTimer;

impl Component for RecurringTimer {
    type Model = ();
    type Message = ();

    fn init(&self) -> Init<(), ()> {
        Init::new(()).with_command(Command::after(std::time::Duration::ZERO, ()))
    }

    fn update(&self, (): &mut (), (): ()) -> Command<()> {
        Command::after(std::time::Duration::from_millis(1), ())
    }
}

#[tokio::test]
async fn shutdown_grace_period_escalates_recurring_timer_and_joins() {
    let mut program = Program::builder();
    program.component(ComponentId::new("recurring-timer"), RecurringTimer);
    let mut task = LiveRuntime::builder(program.build().expect("valid program"))
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
