#![cfg(feature = "fs")]
//! ADR-0027: live Drivers are supplied per capability value, in any order,
//! and completeness is checked when the runtime is built.
use samara::prelude::*;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

/// Appends its configured line once, at startup.
struct Scribe;
impl Component for Scribe {
    type Requires = (File<ReadAppend>,);
    type Config = &'static str;
    type Model = ();
    type Message = ();

    fn init(line: &&'static str, (file,): &Capabilities<Self::Requires>) -> Init<(), ()> {
        Init::new(()).with_command(File::append_line(file, *line).discard_outcome())
    }

    fn update(
        _: &&'static str,
        _: &Capabilities<Self::Requires>,
        (): &mut (),
        (): (),
    ) -> Command<()> {
        Command::none()
    }
}

struct TempRoot(PathBuf);
impl TempRoot {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(std::env::temp_dir().join(format!(
            "samara-capability-binding-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
}
impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

type TwoFiles = (File<ReadAppend>, File<ReadAppend>);

#[tokio::test]
async fn a_driver_follows_its_capability_value_not_a_position() {
    let root = TempRoot::new();
    // The names are assigned against declaration order on purpose: with
    // positional binding this is the silent swap of two same-typed files.
    let (mut builder, (config_file, journal_file)) = Program::builder::<TwoFiles>();
    builder
        .component::<Scribe>("journal")
        .with_capabilities(&(journal_file.clone(),))
        .finish()
        .unwrap();
    builder
        .component::<Scribe>("config")
        .with_capabilities(&(config_file.clone(),))
        .finish()
        .unwrap();

    let runtime = Runtime::builder(builder.build().unwrap())
        .with_capability(journal_file.supplied_by(FileDriver::new(root.0.join("journal.log"))))
        .with_capability(config_file.supplied_by(FileDriver::new(root.0.join("config.log"))))
        .build()
        .expect("every capability has one Driver");

    let task = runtime.spawn();
    assert!(task.shutdown(Shutdown::Drain).await.unwrap().is_clean());
    assert_eq!(
        std::fs::read(root.0.join("journal.log")).unwrap(),
        b"journal\n"
    );
    assert_eq!(
        std::fs::read(root.0.join("config.log")).unwrap(),
        b"config\n"
    );
}

type FileAndClock = (File<ReadAppend>, Effect<ReadSystemTime>);

#[test]
fn a_tuple_is_supplied_whole_or_split_after_registration_borrowed_it() {
    let root = TempRoot::new();

    let (builder, caps) = Program::builder::<FileAndClock>();
    let whole = Runtime::builder(builder.build().unwrap())
        .with_capability(
            caps.supplied_by((FileDriver::new(root.0.join("whole.log")), SystemClockDriver)),
        )
        .build();
    assert!(whole.is_ok());

    let (mut builder, caps) = Program::builder::<FileAndClock>();
    builder
        .component::<Scribe>("split")
        .with_capabilities(&(caps.0.clone(),))
        .finish()
        .unwrap();
    let (file, clock) = &caps;
    let split = Runtime::builder(builder.build().unwrap())
        .with_capability(clock.supplied_by(SystemClockDriver))
        .with_capability(file.supplied_by(FileDriver::new(root.0.join("split.log"))))
        .build();
    assert!(split.is_ok());
}

#[test]
fn a_capability_without_a_driver_fails_the_build_and_is_named() {
    let root = TempRoot::new();
    let (builder, (file, _clock)) = Program::builder::<FileAndClock>();
    let error = Runtime::builder(builder.build().unwrap())
        .with_capability(file.supplied_by(FileDriver::new(root.0.join("only.log"))))
        .build()
        .err()
        .expect("the build is refused")
        .to_string();
    assert!(error.contains("ReadSystemTime"), "{error}");
    assert!(error.contains("0 live bindings"), "{error}");
}

#[test]
fn a_capability_supplied_twice_fails_the_build() {
    let root = TempRoot::new();
    let (builder, (file, clock)) = Program::builder::<FileAndClock>();
    let error = Runtime::builder(builder.build().unwrap())
        .with_capability(file.supplied_by(FileDriver::new(root.0.join("a.log"))))
        .with_capability(file.supplied_by(FileDriver::new(root.0.join("b.log"))))
        .with_capability(clock.supplied_by(SystemClockDriver))
        .build()
        .err()
        .expect("the build is refused")
        .to_string();
    assert!(error.contains("duplicate"), "{error}");
}

#[test]
fn a_capability_from_another_program_fails_the_build() {
    let (_, (foreign_clock,)) = Program::builder::<(Effect<ReadSystemTime>,)>();
    let (builder, (_clock,)) = Program::builder::<(Effect<ReadSystemTime>,)>();
    let error = Runtime::builder(builder.build().unwrap())
        .with_capability(foreign_clock.supplied_by(SystemClockDriver))
        .build()
        .err()
        .expect("the build is refused")
        .to_string();
    assert!(error.contains("ReadSystemTime"), "{error}");
    assert!(error.contains("another Program"), "{error}");
}
