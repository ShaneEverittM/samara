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
}

enum OutputMessage {
    Start,
    Finished(EffectOutcome<(), Infallible>),
    Observed,
}

struct OutputProbe;

/// Whether each finished write is reported through `ObserveOutput`.
struct OutputProbeConfig {
    observe: bool,
}

impl Component for OutputProbe {
    type Io = (
        Effect<PrintStdout>,
        Effect<PrintStderr>,
        Effect<ObserveOutput>,
    );
    type Connections = ();
    type Configuration = OutputProbeConfig;
    type Model = OutputModel;
    type Message = OutputMessage;

    fn init(_: &Context<Self>) -> Init<Self::Model, Self::Message> {
        Init::new(OutputModel::default())
    }

    fn update(
        cx: &Context<Self>,
        model: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        let (stdout, stderr, observe) = &cx.io;
        match message {
            OutputMessage::Start => Command::batch([
                Effect::perform(stdout, PrintStdout::text("")).producing(OutputMessage::Finished),
                Effect::perform(stderr, PrintStderr::text("")).producing(OutputMessage::Finished),
            ]),
            OutputMessage::Finished(outcome) => {
                let EffectOutcome::Succeeded(()) = outcome;
                model.succeeded += 1;
                if cx.configuration.observe {
                    Effect::perform(observe, ObserveOutput(true))
                        .producing(|_| OutputMessage::Observed)
                } else {
                    Command::none()
                }
            }
            OutputMessage::Observed => Command::none(),
        }
    }
}

fn output_program(observe: bool) -> (Program, InstalledComponent<OutputProbe>) {
    let mut builder = Program::builder();
    let probe = builder
        .component_builder::<OutputProbe>()
        .name("standard-output")
        .configuration(OutputProbeConfig { observe })
        .register(());
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
        .control_all()
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
        .complete(stderr, EffectOutcome::Succeeded(()))
        .expect("stderr success accepted");
    runtime.run_until_idle().expect("outcomes mapped");

    assert_eq!(
        runtime.state(&probe).expect("output model"),
        &OutputModel { succeeded: 2 }
    );
    assert!(runtime.cancel().expect("clean controlled close").is_clean());
}

#[tokio::test]
async fn bound_standard_output_drivers_write_and_flush_successfully() {
    let (program, probe) = output_program(true);
    let (observed, mut observations) = tokio::sync::mpsc::unbounded_channel();
    let runtime = Runtime::builder(program)
        .supply(
            &probe.io,
            (
                samara::StdoutDriver::new(),
                samara::StderrDriver::new(),
                OutputObserver {
                    observations: observed,
                },
            ),
        )
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
fn stdio_driver_tuple_rejects_duplicate_bindings() {
    // Drivers are bound per capability (ADR-0028): a second Driver for the
    // same `Effect<PrintStdout>` is the duplicate.
    let (program, probe) = output_program(true);
    let (observed, _observations) = tokio::sync::mpsc::unbounded_channel();
    let error = Runtime::builder(program)
        .supply(
            &probe.io,
            (
                samara::StdoutDriver::new(),
                samara::StderrDriver::new(),
                OutputObserver {
                    observations: observed,
                },
            ),
        )
        .supply(&probe.io.0, DuplicateStdoutDriver)
        .build()
        .err()
        .expect("two Drivers for one capability");
    let error = error.to_string();
    assert!(error.contains("duplicate"), "{error}");
    assert!(error.contains("PrintStdout"), "{error}");
}
