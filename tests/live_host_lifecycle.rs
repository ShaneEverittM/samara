use std::{convert::Infallible, time::Duration};

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

impl Component for FaultProbe {
    type Model = ();
    type Message = FaultMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(())
    }

    fn update(&self, (): &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            FaultMessage::Trigger => Command::effect_discarding_outcome(DynamicallyReachedEffect),
        }
    }
}

#[tokio::test]
async fn run_forever_surfaces_runtime_fault_without_shutdown() {
    let mut program = Program::builder();
    let probe = program.component(ComponentId::new("host-lifecycle-fault"), FaultProbe);
    let runtime = LiveRuntime::builder(program.build().expect("valid program"))
        .build()
        .expect("dynamic work need not be bound at assembly");
    let handle = runtime.handle(&probe).expect("live handle");
    let mut task = runtime.spawn();

    handle
        .send(FaultMessage::Trigger)
        .await
        .expect("trigger accepted before the fault");

    let error = tokio::time::timeout(Duration::from_secs(1), task.run_forever())
        .await
        .expect("runtime termination must become observable immediately")
        .expect_err("the dynamically missing binding must fault the runtime");

    assert_eq!(error.component(), Some(probe.id()));
    assert_eq!(
        error.descriptor_type(),
        Some(std::any::type_name::<DynamicallyReachedEffect>())
    );
    assert_eq!(
        handle.send(FaultMessage::Trigger).await.unwrap_err(),
        error,
        "the lifecycle boundary and later ingress preserve one fault"
    );
    assert_eq!(
        task.shutdown(Shutdown::Cancel).await.unwrap_err(),
        error,
        "later ownership operations preserve the observed terminal fault"
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
