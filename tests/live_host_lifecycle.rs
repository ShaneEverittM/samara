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
