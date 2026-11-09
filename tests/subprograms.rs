//! Acceptance evidence for `Subprogram`, `Requirements`, and `Drivers`: a
//! Subprogram lists its boundaries once as a type; the live builder takes a
//! matching tuple of drivers and the controlled builder takes nothing.

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use samara::prelude::*;

struct Stamp;

impl EffectDescriptor for Stamp {
    type Output = u64;
    type Error = Infallible;
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Pulses;

impl SourceDescriptor for Pulses {
    type Item = u8;
    type Error = Infallible;
}

protocol! {
    pub type Ledger => enum LedgerMessage {
        Record(String),
        Count -> usize,
    }
}

struct Recorder;

struct RecorderCapabilities {
    stamp: EffectCapability<Stamp>,
    pulses: SourceCapability<Pulses>,
}

enum RecorderMessage {
    Ledger(LedgerMessage),
    Stamped(EffectOutcome<u64, Infallible>),
    Pulse(SourceEvent<u8, Infallible>),
}

impl From<LedgerMessage> for RecorderMessage {
    fn from(message: LedgerMessage) -> Self {
        Self::Ledger(message)
    }
}

impl Component for Recorder {
    type Config = RecorderCapabilities;
    type Model = Vec<String>;
    type Message = RecorderMessage;

    fn init(_: &RecorderCapabilities) -> Init<Vec<String>, RecorderMessage> {
        Init::new(Vec::new())
    }

    fn update(
        config: &RecorderCapabilities,
        entries: &mut Vec<String>,
        message: RecorderMessage,
    ) -> Command<RecorderMessage> {
        match message {
            RecorderMessage::Ledger(LedgerMessage::Record(entry)) => {
                entries.push(entry);
                Command::effect_with(&config.stamp, Stamp, RecorderMessage::Stamped)
            }
            RecorderMessage::Ledger(LedgerMessage::Count(call)) => {
                Command::reply(call.reply_to, entries.len())
            }
            RecorderMessage::Stamped(EffectOutcome::Succeeded(stamp)) => {
                entries.push(format!("stamp {stamp}"));
                Command::none()
            }
            RecorderMessage::Pulse(SourceEvent::Item(pulse)) => {
                entries.push(format!("pulse {pulse}"));
                Command::none()
            }
            RecorderMessage::Stamped(_) | RecorderMessage::Pulse(_) => Command::none(),
        }
    }

    fn subscriptions(
        config: &RecorderCapabilities,
        _: &Vec<String>,
    ) -> Subscriptions<RecorderMessage> {
        Subscriptions::one(Subscription::source_with(
            &config.pulses,
            SubscriptionId::new("pulses"),
            Pulses,
            RecorderMessage::Pulse,
        ))
    }
}

struct RecorderConfig {
    port: &'static str,
}

/// Controlled execution has no Port-level ingress, so the test needs the
/// `ComponentRef` as well as the Port. Live hosts only use the Port.
struct RecorderExports {
    ledger: Port<Ledger>,
    recorder: ComponentRef<Recorder>,
}

impl Subprogram for Recorder {
    type Config = RecorderConfig;
    type Exports = RecorderExports;
    type Requires = (Effect<Stamp>, Source<Pulses>);

    fn install(program: &mut ProgramBuilder, config: RecorderConfig) -> RecorderExports {
        let stamp = program.effect::<Stamp>();
        let pulses = program.source::<Pulses>();
        let ledger = program.port::<Ledger>(PortId::new(config.port));
        let recorder = program.component::<Recorder>(
            ComponentId::new("recorder"),
            RecorderCapabilities { stamp, pulses },
        );
        program.bind_port(&ledger, &recorder);
        RecorderExports { ledger, recorder }
    }
}

#[derive(Clone, Default)]
struct CountingStamp {
    executions: Arc<AtomicUsize>,
}

impl EffectDriver<Stamp> for CountingStamp {
    fn execute(&self, _: Stamp) -> BoxFuture<Result<u64, Infallible>> {
        let executions = self.executions.clone();
        Box::pin(async move {
            let count = executions.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(count as u64)
        })
    }
}

struct SilentPulses;

impl SourceDriver<Pulses> for SilentPulses {
    fn run(&self, _: Pulses, _: SourceSink<Pulses>) -> BoxFuture<()> {
        Box::pin(std::future::pending())
    }
}

impl ShipsDrivers for Recorder {
    type Drivers = (CountingStamp, SilentPulses);

    fn drivers() -> Self::Drivers {
        (CountingStamp::default(), SilentPulses)
    }
}

fn assemble() -> (Program, Installed<Recorder>) {
    let mut program = Program::builder();
    let recorder = program.install::<Recorder>(RecorderConfig { port: "ledger" });
    (program.build().expect("program assembles"), recorder)
}

#[test]
fn install_returns_a_token_carrying_the_subprogram_exports() {
    let (_program, recorder) = assemble();
    assert_eq!(recorder.exports().ledger.id(), &PortId::new("ledger"));
    assert!(format!("{recorder:?}").contains("Recorder"));
}

#[tokio::test]
async fn live_builder_binds_every_requirement_through_with_subprogram() {
    let (program, recorder) = assemble();
    let ledger = recorder.exports().ledger.clone();
    let stamp = CountingStamp::default();
    let runtime = LiveRuntime::builder(program)
        .with_subprogram(recorder)
        .and_drivers((stamp.clone(), SilentPulses))
        .build()
        .expect("every requirement is bound");
    let client = runtime.port_handle(&ledger).expect("ledger handle");
    let task = runtime.spawn();

    client
        .notify(Record("first".into()))
        .await
        .expect("notification accepted");
    assert_eq!(client.request(Count).await.expect("count reply"), 1);

    task.shutdown(Shutdown::Drain)
        .await
        .expect("drain completes");
    assert_eq!(stamp.executions.load(Ordering::SeqCst), 1);
}

#[test]
fn live_builder_accepts_the_drivers_a_subprogram_vendors() {
    let (program, recorder) = assemble();
    LiveRuntime::builder(program)
        .with_subprogram(recorder)
        .and_its_vendored_drivers()
        .build()
        .expect("vendored drivers satisfy the requirements");
}

#[test]
fn live_build_fails_when_subprogram_drivers_are_not_bound() {
    let (program, _) = assemble();
    let error = LiveRuntime::builder(program)
        .build()
        .err()
        .expect("unbound drivers are rejected");
    assert!(error.to_string().contains("Stamp"), "{error}");
}

#[test]
fn controlled_builder_registers_every_requirement_through_with_subprogram() {
    let (program, installed) = assemble();
    let recorder = installed.exports().recorder.clone();
    let mut runtime = ControlledRuntime::builder(program)
        .with_subprogram(installed)
        .build()
        .expect("every requirement is controlled");

    runtime.run_until_idle().expect("startup settles");
    runtime
        .send(&recorder, LedgerMessage::Record("first".into()).into())
        .expect("message accepted");
    runtime.run_until_idle().expect("record settles");

    let pending = runtime.next_effect::<Stamp>().expect("stamp requested");
    runtime
        .complete(pending, EffectOutcome::Succeeded(7))
        .expect("outcome delivered");
    runtime.run_until_idle().expect("outcome settles");

    let entries = runtime.state(&recorder).expect("recorder state").clone();
    assert_eq!(entries, ["first", "stamp 7"]);
}

#[test]
fn controlled_build_fails_when_subprogram_drivers_are_not_controlled() {
    let (program, _) = assemble();
    let error = ControlledRuntime::builder(program)
        .build()
        .err()
        .expect("uncontrolled drivers are rejected");
    assert!(error.to_string().contains("Stamp"), "{error}");
}
