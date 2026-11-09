use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

type JournalNeeds = <Journal as Assembly>::Requires;

fn transition() -> JournalMessage {
    JournalMessage::Transition {
        name: "pump".into(),
        from: "idle".into(),
        to: "running".into(),
    }
}

struct EmitTransition;

impl Component for EmitTransition {
    type Requires = ();
    type Config = Connector<JournalMessage>;
    type Model = ();
    type Message = ();

    fn init(journal: &Self::Config, _: &()) -> Init<(), ()> {
        Init::new(()).with_command(Command::send(journal, transition()))
    }

    fn update(_: &Self::Config, _: &(), _: &mut (), _: ()) -> Command<()> {
        Command::none()
    }
}

fn program(config: JournalConfig) -> Program<JournalNeeds> {
    let (mut builder, capabilities) = Program::builder::<JournalNeeds>();
    let journal = builder
        .install::<Journal>(config)
        .with_capabilities(capabilities)
        .finish()
        .unwrap();
    builder
        .component::<EmitTransition>(ComponentId::new("emitter"), journal.connector().clone())
        .finish()
        .unwrap();
    builder.build().unwrap()
}

#[test]
fn assembly_uses_default_and_user_supplied_filenames_in_controlled_appends() {
    for (config, expected) in [
        (JournalConfig::default(), JOURNAL_FILE),
        (
            JournalConfig {
                file: "custom/events.log".into(),
            },
            "custom/events.log",
        ),
    ] {
        let mut runtime = ControlledRuntime::builder(program(config))
            .control_all()
            .build()
            .unwrap();
        runtime.run_until_idle().unwrap();
        let pending = runtime.next_effect::<AppendToFile>().unwrap();
        assert_eq!(pending.intent.path(), Path::new(expected));
        assert_eq!(pending.intent.as_str(), Some("pump: idle -> running\n"));
        runtime
            .complete(pending, EffectOutcome::Succeeded(()))
            .unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(runtime.pending_work().pending_later, 0);
        assert!(runtime.cancel().unwrap().is_clean());
    }
}

#[test]
fn append_outcomes_return_through_messages_without_automatic_retry() {
    let (mut builder, capabilities) = Program::builder::<JournalNeeds>();
    let recorder = builder
        .component::<Recorder>(
            ComponentId::new("recorder"),
            RecorderConfig {
                file: "chosen.log".into(),
            },
        )
        .with_capabilities(capabilities)
        .finish()
        .unwrap();
    let mut runtime = ControlledRuntime::builder(builder.build().unwrap())
        .control_all()
        .build()
        .unwrap();
    let failure = FsError::new(
        FsOperation::Append,
        "chosen.log",
        FsErrorKind::PermissionDenied,
        "read-only fixture",
    );
    for outcome in [
        EffectOutcome::Succeeded(()),
        EffectOutcome::Failed(failure),
        EffectOutcome::Cancelled(CancelReason::Superseded),
    ] {
        let previous = runtime.state(&recorder).unwrap().last_append.clone();
        runtime
            .send(&recorder, Message::Journal(transition()))
            .unwrap();
        runtime.run_until_idle().unwrap();
        let pending = runtime.next_effect::<AppendToFile>().unwrap();
        assert_eq!(runtime.state(&recorder).unwrap().last_append, previous);
        runtime.complete(pending, outcome.clone()).unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(runtime.state(&recorder).unwrap().last_append, Some(outcome));
        assert_eq!(runtime.pending_work().pending_later, 0);
    }
    assert!(runtime.cancel().unwrap().is_clean());
}

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(std::env::temp_dir().join(format!(
            "samara-pulse-config-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        )))
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn standard_driver_uses_the_configured_filename_and_root() {
    let root = TestRoot::new();
    let file = "custom/events.log";
    let runtime = Runtime::builder(program(JournalConfig { file: file.into() }))
        .with_drivers((FsDriver::new(root.0.clone()), SystemClockDriver))
        .build()
        .unwrap();
    assert!(
        !root.0.exists(),
        "configuration and binding must not create files"
    );
    let task = runtime.spawn();
    assert!(task.shutdown(Shutdown::Drain).await.unwrap().is_clean());
    assert_eq!(
        std::fs::read_to_string(root.0.join(file)).unwrap(),
        "pump: idle -> running\n"
    );
    assert!(!root.0.join(JOURNAL_FILE).exists());
}
