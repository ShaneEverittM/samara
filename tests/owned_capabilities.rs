//! ADR-0028: registration creates a Component's capabilities, the installed
//! handle carries them for the host, and each is bound to its own Driver.
use samara::prelude::*;
use std::sync::{Arc, Mutex};

/// Asks the world to note who is asking.
struct Tag(&'static str);
impl EffectDescriptor for Tag {
    type Output = &'static str;
    type Error = !;
}

type Seen = Arc<Mutex<Vec<(&'static str, &'static str)>>>;

/// Records which Driver served which asker, and answers with its own name.
struct NamedDriver {
    name: &'static str,
    seen: Seen,
}
impl EffectDriver<Tag> for NamedDriver {
    fn execute(&self, Tag(asker): Tag) -> BoxFuture<Result<&'static str, !>> {
        self.seen.lock().unwrap().push((self.name, asker));
        let name = self.name;
        Box::pin(async move { Ok(name) })
    }
}

/// Tags itself once at startup and remembers who answered.
struct Asker;
impl Component for Asker {
    type Io = (Effect<Tag>,);
    type Connections = ();
    type Configuration = &'static str;
    type Model = Option<&'static str>;
    type Message = &'static str;

    fn init(cx: &Context<Self>) -> Init<Self::Model, Self::Message> {
        let (tag,) = &cx.io;
        Init::new(None).with_command(
            Effect::perform(tag, Tag(cx.config)).on_outcome(|EffectOutcome::Succeeded(by)| by),
        )
    }

    fn update(
        _: &Context<Self>,
        answered_by: &mut Self::Model,
        by: &'static str,
    ) -> Command<Self::Message> {
        *answered_by = Some(by);
        Command::none()
    }
}

#[tokio::test]
async fn two_components_with_the_same_effect_each_get_their_own_driver() {
    let mut builder = Program::builder();
    let first = builder
        .configure_component::<Asker>()
        .config("first")
        .add(());
    let second = builder
        .configure_component::<Asker>()
        .config("second")
        .add(());

    let seen = Seen::default();
    let runtime = Runtime::builder(builder.build().unwrap())
        .supply(
            &second.io,
            (NamedDriver {
                name: "driver for second",
                seen: seen.clone(),
            },),
        )
        .supply(
            &first.io,
            (NamedDriver {
                name: "driver for first",
                seen: seen.clone(),
            },),
        )
        .build()
        .expect("each capability has its own Driver");

    let task = runtime.spawn();
    assert!(task.shutdown(Shutdown::Drain).await.unwrap().is_clean());

    let mut seen = seen.lock().unwrap().clone();
    seen.sort();
    assert_eq!(
        seen,
        [
            ("driver for first", "first"),
            ("driver for second", "second")
        ]
    );
}

#[test]
fn a_component_left_without_its_driver_fails_the_build() {
    let mut builder = Program::builder();
    let first = builder
        .configure_component::<Asker>()
        .config("first")
        .add(());
    let _second = builder
        .configure_component::<Asker>()
        .config("second")
        .add(());

    let error = Runtime::builder(builder.build().unwrap())
        .supply(
            &first.io,
            (NamedDriver {
                name: "only one",
                seen: Seen::default(),
            },),
        )
        .build()
        .err()
        .expect("the second Component has no Driver")
        .to_string();
    assert!(error.contains("Tag"), "{error}");
    assert!(error.contains("owned_capabilities::Asker"), "{error}");
    assert!(error.contains("0 live bindings"), "{error}");
}

#[test]
fn controlled_execution_controls_capabilities_created_at_registration() {
    let mut builder = Program::builder();
    let first = builder
        .configure_component::<Asker>()
        .config("first")
        .add(());
    let second = builder
        .configure_component::<Asker>()
        .config("second")
        .add(());

    let mut runtime = ControlledRuntime::builder(builder.build().unwrap())
        .control_all()
        .build()
        .expect("every created capability is controlled");
    runtime.run_until_idle().unwrap();

    for answer in ["one", "two"] {
        let pending = runtime.next_effect::<Tag>().expect("a pending Tag");
        runtime
            .complete(pending, EffectOutcome::Succeeded(answer))
            .unwrap();
    }
    runtime.run_until_idle().unwrap();

    assert_eq!(*runtime.state(&first).unwrap(), Some("one"));
    assert_eq!(*runtime.state(&second).unwrap(), Some("two"));
}

/// A one-Component Assembly: its capabilities are its member's.
struct Tagging;
impl Assembly for Tagging {
    type Config = &'static str;
    type Exports = Connector<&'static str>;
    type Io = (Effect<Tag>,);

    fn install(program: &mut ProgramBuilder, label: &'static str) -> (Self::Exports, Self::Io) {
        let asker = program.configure_component::<Asker>().config(label).add(());
        (asker.connector(), asker.io.clone())
    }
}

#[tokio::test]
async fn an_installed_assembly_carries_its_capabilities_for_the_host() {
    let mut builder = Program::builder();
    let left = builder.add_assembly::<Tagging>("left");
    let right = builder.add_assembly::<Tagging>("right");

    let seen = Seen::default();
    let runtime = Runtime::builder(builder.build().unwrap())
        .supply(
            &left.io,
            (NamedDriver {
                name: "left driver",
                seen: seen.clone(),
            },),
        )
        .supply(
            &right.io,
            (NamedDriver {
                name: "right driver",
                seen: seen.clone(),
            },),
        )
        .build()
        .expect("each installation has its own Driver");

    let task = runtime.spawn();
    assert!(task.shutdown(Shutdown::Drain).await.unwrap().is_clean());

    let mut seen = seen.lock().unwrap().clone();
    seen.sort();
    assert_eq!(seen, [("left driver", "left"), ("right driver", "right")]);
}

struct Quiet;
impl Component for Quiet {
    type Io = ();
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

#[test]
fn a_component_with_no_capabilities_is_installed_like_any_other() {
    let mut builder = Program::builder();
    let quiet: InstalledComponent<Quiet> = builder.add_component::<Quiet>(());
    let asker: InstalledComponent<Asker> = builder
        .configure_component::<Asker>()
        .config("asker")
        .add(());
    let () = quiet.io;
    // The handle is an address and offers what a ComponentRef does.
    let _: Connector<&'static str> = asker.connector();
    let _: &ComponentRef<Asker> = asker.reference();
    let _: &ComponentRef<Quiet> = quiet.reference();
    assert_ne!(quiet.id(), asker.id());
    // Nothing needs a Driver on Quiet's account.
    let error = Runtime::builder(builder.build().unwrap())
        .build()
        .err()
        .expect("only the Asker's Tag is unsupplied")
        .to_string();
    assert!(error.contains("owned_capabilities::Asker"), "{error}");
}

#[test]
fn a_program_lists_every_capability_it_declared_and_who_owns_it() {
    let mut builder = Program::builder();
    builder
        .configure_component::<Asker>()
        .name("first")
        .config("first")
        .add(());
    builder.add_component::<Quiet>(());
    builder.add_assembly::<Tagging>("left");
    let manifest = builder.build().unwrap().manifest();

    let owners: Vec<_> = manifest
        .capabilities()
        .iter()
        .map(|entry| (entry.owner.as_str(), entry.name.as_deref(), entry.kind))
        .collect();
    assert_eq!(
        owners,
        [
            ("first", None, CapabilityKind::Effect),
            ("owned_capabilities::Asker", None, CapabilityKind::Effect),
        ]
    );
    assert_eq!(
        manifest.to_string(),
        "first: effect owned_capabilities::Tag\n\
         owned_capabilities::Asker: effect owned_capabilities::Tag\n"
    );
}

/// Ongoing ticks from the world.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Ticks;
impl SourceDescriptor for Ticks {
    type Terminal = Self;
    type Item = u8;
    type Error = !;
}

struct SilentTicks;
impl SourceDriver<Ticks> for SilentTicks {
    fn run(&self, _: Ticks, _: SourceSink<Ticks>) -> BoxFuture<()> {
        Box::pin(std::future::pending())
    }
}

/// Two capabilities, each with a name.
#[derive(Clone, Io)]
struct DeskIo {
    tag: Effect<Tag>,
    ticks: Source<Ticks>,
}

struct Desk;
impl Component for Desk {
    type Io = DeskIo;
    type Connections = ();
    type Configuration = ();
    type Model = ();
    type Message = SourceEvent<u8, !>;

    fn init(cx: &Context<Self>) -> Init<(), Self::Message> {
        Init::new(()).with_command(Effect::perform(&cx.io.tag, Tag("desk")).discard())
    }

    fn update(_: &Context<Self>, (): &mut (), _: Self::Message) -> Command<Self::Message> {
        Command::none()
    }

    fn subscriptions(cx: &Context<Self>, (): &()) -> Subscriptions<Self::Message> {
        Subscriptions::one(Subscription::source(&cx.io.ticks, Ticks))
    }
}

fn desk_program() -> (Program, InstalledComponent<Desk>) {
    let mut builder = Program::builder();
    let desk = builder.configure_component::<Desk>().name("desk").add(());
    (builder.build().unwrap(), desk)
}

#[test]
fn every_capability_in_the_manifest_has_an_owner() {
    // ADR-0028: there is no up-front declaration, so no capability is
    // ownerless. Named fields add the field to the owner.
    let (program, _desk) = desk_program();
    let manifest = program.manifest();
    assert_eq!(
        manifest
            .capabilities()
            .iter()
            .map(|entry| (entry.owner.as_str(), entry.name.as_deref(), entry.kind))
            .collect::<Vec<_>>(),
        [
            ("desk", Some("tag"), CapabilityKind::Effect),
            ("desk", Some("ticks"), CapabilityKind::Source),
        ]
    );
    assert_eq!(
        manifest.to_string(),
        "desk.tag: effect owned_capabilities::Tag\n\
         desk.ticks: source owned_capabilities::Ticks\n"
    );
}

#[test]
fn a_missing_driver_for_a_named_field_is_reported_as_owner_dot_field() {
    let (program, _desk) = desk_program();
    let error = Runtime::builder(program)
        .build()
        .err()
        .expect("nothing was supplied")
        .to_string();
    assert!(error.contains("Effect capability desk.tag"), "{error}");
    assert!(error.contains("0 live bindings"), "{error}");

    let (program, desk) = desk_program();
    let error = Runtime::builder(program)
        .supply(
            &desk.io.tag,
            NamedDriver {
                name: "desk driver",
                seen: Seen::default(),
            },
        )
        .build()
        .err()
        .expect("the ticks Source has no Driver")
        .to_string();
    assert!(error.contains("Source capability desk.ticks"), "{error}");
    assert!(error.contains("0 live bindings"), "{error}");

    let (program, desk) = desk_program();
    Runtime::builder(program)
        .supply(
            &desk.io.tag,
            NamedDriver {
                name: "desk driver",
                seen: Seen::default(),
            },
        )
        .supply(&desk.io.ticks, DrivenSource::new(SilentTicks))
        .build()
        .expect("both named capabilities are supplied");
}

/// Owns a Tag of its own but, when told to, uses one that was created for
/// another Component and smuggled in through Config.
struct Borrower;
impl Component for Borrower {
    type Io = (Effect<Tag>,);
    type Connections = ();
    type Configuration = (Effect<Tag>,);
    type Model = ();
    type Message = ();

    fn init(_: &Context<Self>) -> Init<(), ()> {
        Init::new(())
    }

    fn update(cx: &Context<Self>, (): &mut (), (): ()) -> Command<()> {
        let (theirs,) = &cx.config;
        Effect::perform(theirs, Tag("borrower")).discard()
    }
}

fn borrowing_program() -> (
    Program,
    InstalledComponent<Asker>,
    InstalledComponent<Borrower>,
) {
    let mut builder = Program::builder();
    let owner = builder
        .configure_component::<Asker>()
        .config("owner")
        .add(());
    let borrower = builder
        .configure_component::<Borrower>()
        .config(owner.io.clone())
        .add(());
    (builder.build().unwrap(), owner, borrower)
}

#[test]
fn using_another_components_capability_faults_in_controlled_execution() {
    let (program, _owner, borrower) = borrowing_program();
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
        .build()
        .expect("every capability is controlled");
    runtime.run_until_idle().unwrap();
    let owners_own = runtime
        .next_effect::<Tag>()
        .expect("the owner's startup Tag");
    assert_eq!(owners_own.intent.0, "owner");

    runtime.send(&borrower, ()).unwrap();
    let error = runtime
        .run_until_idle()
        .expect_err("the capability was created for the owner")
        .to_string();
    assert!(error.contains("belongs to another Component"), "{error}");
    assert!(runtime.next_effect::<Tag>().is_err());
}

#[tokio::test]
async fn using_another_components_capability_faults_in_live_execution() {
    let (program, owner, borrower) = borrowing_program();
    let seen = Seen::default();
    let runtime = Runtime::builder(program)
        .supply(
            &owner.io,
            (NamedDriver {
                name: "owner driver",
                seen: seen.clone(),
            },),
        )
        .supply(
            &borrower.io,
            (NamedDriver {
                name: "borrower driver",
                seen: seen.clone(),
            },),
        )
        .build()
        .expect("each capability has its own Driver");
    let handle = runtime.handle(&borrower).unwrap();
    let mut task = runtime.spawn();
    handle.send(()).await.unwrap();
    let error = tokio::time::timeout(std::time::Duration::from_secs(2), task.run_forever())
        .await
        .expect("the fault must surface promptly")
        .expect_err("the capability was created for the owner")
        .to_string();
    assert!(error.contains("belongs to another Component"), "{error}");
    // Neither Driver ran on the Borrower's behalf.
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .all(|(_, asker)| *asker != "borrower")
    );
}
