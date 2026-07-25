use std::convert::Infallible;

use samara::prelude::*;

#[derive(Clone, Debug)]
struct ObserveOutput(bool);

impl EffectDescriptor for ObserveOutput {
    type Output = ();
    type Error = Infallible;
}

struct OutputObserver {
    observations: tokio::sync::mpsc::UnboundedSender<bool>,
}

impl EffectDriver<ObserveOutput> for OutputObserver {
    fn execute(&self, descriptor: ObserveOutput) -> BoxFuture<Result<(), Infallible>> {
        let observations = self.observations.clone();
        Box::pin(async move {
            let _ = observations.send(descriptor.0);
            Ok(())
        })
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct OutputModel {
    succeeded: usize,
    cancelled: usize,
}

enum OutputMessage {
    Start,
    Finished(EffectOutcome<(), Infallible>),
    Observed,
}

struct OutputProbe {
    observe: bool,
}

impl Component for OutputProbe {
    type Model = OutputModel;
    type Message = OutputMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(OutputModel::default())
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            OutputMessage::Start => Command::batch([
                Command::effect(PrintStdout::text(""), OutputMessage::Finished),
                Command::effect(PrintStderr::text(""), OutputMessage::Finished),
            ]),
            OutputMessage::Finished(outcome) => {
                let succeeded = match outcome {
                    EffectOutcome::Succeeded(()) => {
                        model.succeeded += 1;
                        true
                    }
                    EffectOutcome::Failed(never) => match never {},
                    EffectOutcome::Cancelled(_) => {
                        model.cancelled += 1;
                        false
                    }
                };
                if self.observe {
                    Command::effect(ObserveOutput(succeeded), |_| OutputMessage::Observed)
                } else {
                    Command::none()
                }
            }
            OutputMessage::Observed => Command::none(),
        }
    }
}

fn output_program(observe: bool) -> (Program, ComponentRef<OutputProbe>) {
    let mut builder = Program::builder();
    let probe = builder.component(ComponentId::new("standard-output"), OutputProbe { observe });
    (builder.build().expect("valid output program"), probe)
}

#[test]
fn standard_output_descriptors_preserve_exact_text_and_lines() {
    fn assert_output_effect<D>()
    where
        D: EffectDescriptor<Output = (), Error = Infallible>,
    {
    }

    assert_output_effect::<PrintStdout>();
    assert_output_effect::<PrintStderr>();

    assert_eq!(PrintStdout::text("hello").as_str(), "hello");
    assert_eq!(PrintStderr::line("failure").as_str(), "failure\n");
    assert_eq!(PrintStdout::line("already\n").as_str(), "already\n\n");
}

#[test]
fn standard_output_effects_are_ordinary_controlled_boundaries() {
    let (program, probe) = output_program(false);
    let mut runtime = ControlledRuntime::builder(program)
        .control_effect::<PrintStdout>()
        .control_effect::<PrintStderr>()
        .build()
        .expect("valid controlled output bindings");

    runtime
        .send(&probe, OutputMessage::Start)
        .expect("start accepted");
    runtime.run_until_idle().expect("writes intercepted");

    let stdout = runtime
        .next_effect::<PrintStdout>()
        .expect("typed stdout write");
    let stderr = runtime
        .next_effect::<PrintStderr>()
        .expect("typed stderr write");
    assert!(stdout.intent.as_str().is_empty());
    assert!(stderr.intent.as_str().is_empty());

    runtime
        .complete(stdout, EffectOutcome::Succeeded(()))
        .expect("stdout success accepted");
    runtime
        .complete(stderr, EffectOutcome::Cancelled(CancelReason::Superseded))
        .expect("stderr cancellation accepted");
    runtime.run_until_idle().expect("outcomes mapped");

    assert_eq!(
        runtime.state(&probe).expect("output model"),
        &OutputModel {
            succeeded: 1,
            cancelled: 1,
        }
    );
    assert!(runtime.cancel().expect("clean controlled close").is_clean());
}

#[tokio::test]
async fn bound_standard_output_drivers_write_and_flush_successfully() {
    let (program, probe) = output_program(true);
    let (observed, mut observations) = tokio::sync::mpsc::unbounded_channel();
    let runtime = LiveRuntime::builder(program)
        .bind_stdio()
        .bind_effect::<ObserveOutput, _>(OutputObserver {
            observations: observed,
        })
        .build()
        .expect("valid live output bindings");
    let handle = runtime.handle(&probe).expect("registered output probe");
    let task = runtime.spawn();

    handle
        .send(OutputMessage::Start)
        .await
        .expect("start accepted");
    assert_eq!(observations.recv().await, Some(true));
    assert_eq!(observations.recv().await, Some(true));
    assert!(task.shutdown(Shutdown::Drain).await.unwrap().is_clean());
}

struct DuplicateStdoutDriver;

impl EffectDriver<PrintStdout> for DuplicateStdoutDriver {
    fn execute(&self, _descriptor: PrintStdout) -> BoxFuture<Result<(), Infallible>> {
        Box::pin(async { Ok(()) })
    }
}

#[test]
fn bind_stdio_participates_in_normal_duplicate_binding_validation() {
    let (program, _) = output_program(false);
    assert!(
        LiveRuntime::builder(program)
            .bind_stdio()
            .bind_effect::<PrintStdout, _>(DuplicateStdoutDriver)
            .build()
            .is_err()
    );
}
