//! ADR-0028: registration creates a Component's capabilities, the installed
//! handle carries them for the host, and each is bound to its own Driver.
use samara::prelude::*;
use std::{
    convert::Infallible,
    sync::{Arc, Mutex},
};

/// Asks the world to note who is asking.
struct Tag(&'static str);
impl EffectDescriptor for Tag {
    type Output = &'static str;
    type Error = Infallible;
}

type Seen = Arc<Mutex<Vec<(&'static str, &'static str)>>>;

/// Records which Driver served which asker, and answers with its own name.
struct NamedDriver {
    name: &'static str,
    seen: Seen,
}
impl EffectDriver<Tag> for NamedDriver {
    fn execute(&self, Tag(asker): Tag) -> BoxFuture<Result<&'static str, Infallible>> {
        self.seen.lock().unwrap().push((self.name, asker));
        let name = self.name;
        Box::pin(async move { Ok(name) })
    }
}

/// Tags itself once at startup and remembers who answered.
struct Asker;
impl Component for Asker {
    type Requires = (Effect<Tag>,);
    type Config = &'static str;
    type Model = Option<&'static str>;
    type Message = &'static str;

    fn init(
        label: &&'static str,
        (tag,): &Capabilities<Self::Requires>,
    ) -> Init<Self::Model, Self::Message> {
        Init::new(None).with_command(
            Effect::perform(tag, Tag(label)).producing(|EffectOutcome::Succeeded(by)| by),
        )
    }

    fn update(
        _: &&'static str,
        _: &Capabilities<Self::Requires>,
        answered_by: &mut Self::Model,
        by: &'static str,
    ) -> Command<Self::Message> {
        *answered_by = Some(by);
        Command::none()
    }
}

#[tokio::test]
async fn two_components_with_the_same_effect_each_get_their_own_driver() {
    let mut builder = ProgramBuilder::new();
    let first = builder.component::<Asker>("first").finish().unwrap();
    let second = builder.component::<Asker>("second").finish().unwrap();

    let seen = Seen::default();
    let runtime = Runtime::builder(builder.build().unwrap())
        .with(second.capabilities())
        .supplied_by((NamedDriver {
            name: "driver for second",
            seen: seen.clone(),
        },))
        .with(first.capabilities())
        .supplied_by((NamedDriver {
            name: "driver for first",
            seen: seen.clone(),
        },))
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
    let mut builder = ProgramBuilder::new();
    let first = builder.component::<Asker>("first").finish().unwrap();
    let _second = builder.component::<Asker>("second").finish().unwrap();

    let error = Runtime::builder(builder.build().unwrap())
        .with(first.capabilities())
        .supplied_by((NamedDriver {
            name: "only one",
            seen: Seen::default(),
        },))
        .build()
        .err()
        .expect("the second Component has no Driver")
        .to_string();
    assert!(error.contains("Tag"), "{error}");
    assert!(error.contains("0 live bindings"), "{error}");
}

#[test]
fn controlled_execution_controls_capabilities_created_at_registration() {
    let mut builder = ProgramBuilder::new();
    let first = builder.component::<Asker>("first").finish().unwrap();
    let second = builder.component::<Asker>("second").finish().unwrap();

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

/// A one-Component Assembly, written as Assemblies are today.
struct Tagging;
impl Assembly for Tagging {
    type Config = &'static str;
    type Exports = Connector<&'static str>;
    type Requires = (Effect<Tag>,);

    fn install<R: Requirements>(
        program: &mut ProgramBuilder<R>,
        label: &'static str,
        capabilities: Capabilities<Self::Requires>,
    ) -> Result<Self::Exports, ProgramBuildError> {
        let asker = program
            .component::<Asker>(label)
            .with(&capabilities)
            .finish()?;
        Ok(asker.connector())
    }
}

#[tokio::test]
async fn an_installed_assembly_carries_its_capabilities_for_the_host() {
    let mut builder = ProgramBuilder::new();
    let left = builder.install::<Tagging>("left").finish().unwrap();
    let right = builder.install::<Tagging>("right").finish().unwrap();

    let seen = Seen::default();
    let runtime = Runtime::builder(builder.build().unwrap())
        .with(left.capabilities())
        .supplied_by((NamedDriver {
            name: "left driver",
            seen: seen.clone(),
        },))
        .with(right.capabilities())
        .supplied_by((NamedDriver {
            name: "right driver",
            seen: seen.clone(),
        },))
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
    type Requires = ();
    type Config = ();
    type Model = ();
    type Message = ();

    fn init(_: &(), _: &Capabilities<()>) -> Init<(), ()> {
        Init::new(())
    }

    fn update(_: &(), _: &Capabilities<()>, (): &mut (), (): ()) -> Command<()> {
        Command::none()
    }
}

#[test]
fn a_component_that_requires_nothing_returns_its_plain_reference() {
    let mut builder = ProgramBuilder::new();
    let quiet: ComponentRef<Quiet> = builder.component::<Quiet>(()).finish().unwrap();
    let asker: InstalledComponent<Asker> = builder.component::<Asker>("asker").finish().unwrap();
    // The handle is an address and offers what a ComponentRef does.
    let _: Connector<&'static str> = asker.connector();
    let _: &ComponentRef<Asker> = asker.reference();
    assert_ne!(quiet.id(), asker.id());
}
