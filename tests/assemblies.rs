//! Acceptance evidence for `Assembly`, `Requirements`, and `Drivers`: a
//! Assembly lists its boundaries once as a type; the live builder takes a
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
    type Terminal = Self;
    type Item = u8;
    type Error = Infallible;
}

/// The Assembly's exported vocabulary, written by hand: the request variant
/// carries its own reply token.
pub enum LedgerMessage {
    Record(String),
    Count(ReplyWith<usize>),
}

struct Recorder;

type RecorderNeeds = (Effect<Stamp>, Source<Pulses>);

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
    type Requires = RecorderNeeds;
    type Config = ();
    type Model = Vec<String>;
    type Message = RecorderMessage;

    fn init(_: &(), _: &Capabilities<RecorderNeeds>) -> Init<Vec<String>, RecorderMessage> {
        Init::new(Vec::new())
    }

    fn update(
        _: &(),
        (stamp, _pulses): &Capabilities<RecorderNeeds>,
        entries: &mut Vec<String>,
        message: RecorderMessage,
    ) -> Command<RecorderMessage> {
        match message {
            RecorderMessage::Ledger(LedgerMessage::Record(entry)) => {
                entries.push(entry);
                Command::effect_with(stamp, Stamp, RecorderMessage::Stamped)
            }
            RecorderMessage::Ledger(LedgerMessage::Count(reply_with)) => {
                Command::reply(reply_with, entries.len())
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
        _: &(),
        (_stamp, pulses): &Capabilities<RecorderNeeds>,
        _: &Vec<String>,
    ) -> Subscriptions<RecorderMessage> {
        Subscriptions::one(Subscription::source_with(
            pulses,
            SubscriptionId::new("pulses"),
            Pulses,
            RecorderMessage::Pulse,
        ))
    }
}

/// Controlled execution has no Port-level ingress, so the test keeps the
/// `ComponentRef` in `Exports`. Live hosts only use the Connector.
struct RecorderExports {
    recorder: ComponentRef<Recorder>,
}

impl Assembly for Recorder {
    type Config = ();
    type Accepts = LedgerMessage;
    type Exports = RecorderExports;
    type Requires = (Effect<Stamp>, Source<Pulses>);

    fn install<R: Requirements>(
        program: &mut ProgramBuilder<R>,
        _: (),
        (stamp, pulses): Capabilities<RecorderNeeds>,
    ) -> Result<(Connector<LedgerMessage>, RecorderExports), ProgramBuildError> {
        let recorder = program
            .component::<Recorder>(ComponentId::new("recorder"), ())
            .with_capabilities((stamp, pulses))
            .finish()?;
        Ok((
            recorder.accepting(RecorderMessage::Ledger),
            RecorderExports { recorder },
        ))
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

impl VendorsDrivers for Recorder {
    type DriverConfig = Arc<AtomicUsize>;
    type Drivers = (CountingStamp, DrivenSource<SilentPulses>);

    fn drivers(executions: Self::DriverConfig) -> Self::Drivers {
        (
            CountingStamp { executions },
            DrivenSource::new(SilentPulses),
        )
    }
}

/// The host names its own Port and connects the Assembly's Connector to it.
fn assemble() -> (
    Program<RecorderNeeds>,
    InstalledAssembly<Recorder>,
    Port<LedgerMessage>,
) {
    let (mut program, caps) = Program::builder::<RecorderNeeds>();
    let recorder = program
        .install::<Recorder>(())
        .with_capabilities(caps)
        .finish()
        .expect("assembly installation");
    let ledger = program.port::<LedgerMessage>(PortId::new("ledger"));
    program.connect(&ledger, recorder.connector());
    (
        program.build().expect("program assembles"),
        recorder,
        ledger,
    )
}

#[test]
fn install_returns_a_token_carrying_the_connector_and_exports() {
    let (_program, recorder, _) = assemble();
    assert_eq!(recorder.connector().id(), &ComponentId::new("recorder"));
    assert_eq!(
        recorder.exports().recorder.id(),
        &ComponentId::new("recorder")
    );
    assert!(format!("{recorder:?}").contains("Recorder"));
}

#[tokio::test]
async fn live_builder_binds_every_requirement_through_with_assembly() {
    let (program, _recorder, ledger) = assemble();
    let stamp = CountingStamp::default();
    let runtime = Runtime::builder(program)
        .with_drivers((stamp.clone(), DrivenSource::new(SilentPulses)))
        .build()
        .expect("every requirement is bound");
    let client = runtime.port_handle(&ledger).expect("ledger handle");
    let task = runtime.spawn();

    client
        .send(LedgerMessage::Record("first".into()))
        .await
        .expect("send accepted");
    assert_eq!(
        client
            .request_to(LedgerMessage::Count)
            .await
            .expect("count reply"),
        1
    );

    task.shutdown(Shutdown::Drain)
        .await
        .expect("drain completes");
    assert_eq!(stamp.executions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn live_builder_accepts_the_drivers_a_assembly_vendors() {
    let (program, _recorder, ledger) = assemble();
    let executions = Arc::new(AtomicUsize::new(7));
    let runtime = Runtime::builder(program)
        .with_drivers(Recorder::drivers(executions.clone()))
        .build()
        .expect("vendored drivers satisfy the requirements");
    let client = runtime.port_handle(&ledger).unwrap();
    let task = runtime.spawn();
    client
        .send(LedgerMessage::Record("configured".into()))
        .await
        .unwrap();
    assert!(task.shutdown(Shutdown::Drain).await.unwrap().is_clean());
    assert_eq!(executions.load(Ordering::SeqCst), 8);
}

// Missing-binding coverage moved to RuntimeSetup compile-fail contracts (ADR-0019).

#[test]
fn controlled_builder_registers_every_requirement_through_with_assembly() {
    let (program, installed, _) = assemble();
    let recorder = installed.exports().recorder.clone();
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
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

// Missing-binding coverage moved to RuntimeSetup compile-fail contracts (ADR-0019).
