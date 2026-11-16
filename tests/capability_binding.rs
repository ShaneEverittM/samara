#![cfg(feature = "fs")]
//! ADR-0027: live Drivers are supplied per capability value, in any order,
//! and completeness is checked when the runtime is built. The capability
//! values are the ones registration created for each Component (ADR-0028).
use samara::prelude::*;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

/// Appends its configured line once, at startup.
struct Scribe;
impl Component for Scribe {
    type Io = (File<ReadAppend>,);
    type Connections = ();
    type Configuration = &'static str;
    type Model = ();
    type Message = ();

    fn init(cx: &Context<Self>) -> Init<(), ()> {
        let (file,) = &cx.io;
        Init::new(()).with_command(File::append_line(file, cx.configuration).discard_outcome())
    }

    fn update(_: &Context<Self>, (): &mut (), (): ()) -> Command<()> {
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

#[tokio::test]
async fn a_driver_follows_its_capability_value_not_a_position() {
    let root = TempRoot::new();
    // The Drivers are supplied against registration order on purpose: with
    // positional binding this is the silent swap of two same-typed files.
    let mut builder = Program::builder();
    let journal = builder
        .configure_component::<Scribe>()
        .configuration("journal")
        .add(());
    let config = builder
        .configure_component::<Scribe>()
        .configuration("config")
        .add(());

    let runtime = Runtime::builder(builder.build().unwrap())
        .supply(&config.io.0, FileDriver::new(root.0.join("config.log")))
        .supply(&journal.io.0, FileDriver::new(root.0.join("journal.log")))
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

/// Owns a file and a clock and uses neither: the tests below are about what
/// the host must supply, not about what the Component does.
struct Keeper;
impl Component for Keeper {
    type Io = FileAndClock;
    type Connections = ();
    type Configuration = ();
    type Model = ();
    type Message = ();

    fn init(_: &Context<Self>) -> Init<(), ()> {
        Init::new(())
    }

    fn update(_: &Context<Self>, (): &mut (), (): ()) -> Command<()> {
        Command::none()
    }
}

fn keeper_program() -> (Program, InstalledComponent<Keeper>) {
    let mut builder = Program::builder();
    let keeper = builder
        .configure_component::<Keeper>()
        .name("keeper")
        .add(());
    (builder.build().unwrap(), keeper)
}

#[test]
fn a_tuple_is_supplied_whole_or_split() {
    let root = TempRoot::new();

    let (program, keeper) = keeper_program();
    let whole = Runtime::builder(program)
        .supply(
            &keeper.io,
            (FileDriver::new(root.0.join("whole.log")), SystemClockDriver),
        )
        .build();
    assert!(whole.is_ok());

    let (program, keeper) = keeper_program();
    let (file, clock) = &keeper.io;
    let split = Runtime::builder(program)
        .supply(clock, SystemClockDriver)
        .supply(file, FileDriver::new(root.0.join("split.log")))
        .build();
    assert!(split.is_ok());
}

#[test]
fn a_capability_without_a_driver_fails_the_build_and_is_named() {
    let root = TempRoot::new();
    let (program, keeper) = keeper_program();
    let error = Runtime::builder(program)
        .supply(&keeper.io.0, FileDriver::new(root.0.join("only.log")))
        .build()
        .err()
        .expect("the build is refused")
        .to_string();
    assert!(error.contains("keeper"), "{error}");
    assert!(error.contains("ReadSystemTime"), "{error}");
    assert!(error.contains("0 live bindings"), "{error}");
}

#[test]
fn a_capability_supplied_twice_fails_the_build() {
    let root = TempRoot::new();
    let (program, keeper) = keeper_program();
    let (file, clock) = &keeper.io;
    let error = Runtime::builder(program)
        .supply(file, FileDriver::new(root.0.join("a.log")))
        .supply(file, FileDriver::new(root.0.join("b.log")))
        .supply(clock, SystemClockDriver)
        .build()
        .err()
        .expect("the build is refused")
        .to_string();
    assert!(error.contains("duplicate"), "{error}");
}

#[test]
fn a_capability_from_another_program_fails_the_build() {
    let (_, foreign) = keeper_program();
    let (program, _own) = keeper_program();
    let error = Runtime::builder(program)
        .supply(&foreign.io.1, SystemClockDriver)
        .build()
        .err()
        .expect("the build is refused")
        .to_string();
    assert!(error.contains("ReadSystemTime"), "{error}");
    assert!(error.contains("another Program"), "{error}");
}
