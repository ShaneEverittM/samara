//! ADR-0019 acceptance: the Program owns its typed binding obligations.
//! ADR-0028: each obligation is a capability created for one Component.
use samara::prelude::*;
use std::convert::Infallible;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Read;
impl EffectDescriptor for Read {
    type Output = u64;
    type Error = Infallible;
}
// This probe intentionally instruments init through Config to observe call timing.
// Application state belongs in Model.
struct Reader;
impl Component for Reader {
    type Connections = ();
    type Configuration = Arc<AtomicUsize>;
    type Io = (Effect<Read>,);
    type Model = Vec<u64>;
    type Message = Option<u64>;
    fn init(cx: &Context<Self>) -> Init<Self::Model, Self::Message> {
        cx.config.fetch_add(1, Ordering::SeqCst);
        Init::default()
    }
    fn update(
        cx: &Context<Self>,
        model: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        let (read,) = &cx.io;
        if let Some(value) = message {
            model.push(value);
            Command::none()
        } else {
            Effect::perform(read, Read).producing(|outcome| match outcome {
                EffectOutcome::Succeeded(value) => Some(value),
                _ => None,
            })
        }
    }
}
struct ReadDriver;
impl EffectDriver<Read> for ReadDriver {
    fn execute(&self, _: Read) -> BoxFuture<Result<u64, Infallible>> {
        Box::pin(async { Ok(42) })
    }
}

/// Owns capabilities and never uses them: for tests about what a Program
/// obliges its host to supply.
struct Holds<C>(std::marker::PhantomData<fn() -> C>);
impl<C: Io> Component for Holds<C> {
    type Connections = ();
    type Configuration = ();
    type Io = C;
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
fn each_components_own_capability_reaches_callbacks_and_delayed_effects() {
    let mut program = Program::builder();
    let count = Arc::new(AtomicUsize::new(0));
    let first = program
        .configure_component::<Reader>()
        .name("first")
        .configuration(count.clone())
        .add(());
    let second = program
        .configure_component::<Reader>()
        .name("second")
        .configuration(count.clone())
        .add(());
    assert_eq!(count.load(Ordering::SeqCst), 2);
    let mut runtime = ControlledRuntime::builder(program.build().unwrap())
        .control_all()
        .build()
        .unwrap();
    for reader in [&first, &second] {
        runtime.send(reader, None).unwrap();
        runtime.run_until_idle().unwrap();
        let effect = runtime.next_effect::<Read>().unwrap();
        runtime
            .complete(effect, EffectOutcome::Succeeded(42))
            .unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(runtime.state(reader).unwrap(), &[42]);
    }
    assert!(runtime.cancel().unwrap().is_clean());
}

#[tokio::test]
async fn live_drivers_are_supplied_per_component_capability() {
    fn readers() -> (
        Program,
        InstalledComponent<Reader>,
        InstalledComponent<Reader>,
    ) {
        let mut program = Program::builder();
        let count = Arc::new(AtomicUsize::new(0));
        let first = program
            .configure_component::<Reader>()
            .name("first")
            .configuration(count.clone())
            .add(());
        let second = program
            .configure_component::<Reader>()
            .name("second")
            .configuration(count)
            .add(());
        (program.build().unwrap(), first, second)
    }
    // One Driver no longer serves every capability of its descriptor type.
    let (program, first, _second) = readers();
    let error = Runtime::builder(program)
        .supply(&first.io.0, ReadDriver)
        .build()
        .err()
        .expect("the second Reader's capability has no Driver")
        .to_string();
    assert!(error.contains("second"), "{error}");

    let (program, first, second) = readers();
    let runtime = Runtime::builder(program)
        .supply(&first.io, (ReadDriver,))
        .supply(&second.io, (ReadDriver,))
        .build()
        .unwrap();
    let handle = runtime.handle(&first).unwrap();
    let task = runtime.spawn();
    handle.send(None).await.unwrap();
    assert!(task.shutdown(Shutdown::Drain).await.unwrap().is_clean());
}

#[test]
fn two_exact_stream_positions_remain_distinct() {
    type Inputs = (Source<StreamDescriptor<u64>>, Source<StreamDescriptor<u64>>);
    let mut program = Program::builder();
    let holder = program.add_component::<Holds<Inputs>>(());
    let (_, first) = tokio::sync::mpsc::channel(1);
    let (_, second) = tokio::sync::mpsc::channel(1);
    let tokio = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _guard = tokio.enter();
    Runtime::builder(program.build().unwrap())
        .supply(
            &holder.io,
            (MpscReceiver::new(first), MpscReceiver::new(second)),
        )
        .build()
        .unwrap();
    let mut program = Program::builder();
    program.add_component::<Holds<Inputs>>(());
    ControlledRuntime::builder(program.build().unwrap())
        .control_all()
        .build()
        .unwrap();
}

struct Library;
impl Assembly for Library {
    type Config = Arc<AtomicUsize>;
    type Exports = ComponentRef<Reader>;
    type Io = (Effect<Read>,);
    fn install(program: &mut ProgramBuilder, count: Self::Config) -> (Self::Exports, Self::Io) {
        let reader = program
            .configure_component::<Reader>()
            .name("library")
            .configuration(count)
            .add(());
        let io = reader.io.clone();
        (reader.into(), io)
    }
}

#[test]
fn repeated_assembly_installations_need_no_component_name_coordination() {
    let mut program = Program::builder();
    let count = Arc::new(AtomicUsize::new(0));
    let first = program.add_assembly::<Library>(count.clone());
    let second = program.add_assembly::<Library>(count.clone());
    assert_eq!(first.exports().id().name(), second.exports().id().name());
    assert_ne!(first.exports().id(), second.exports().id());
    assert_eq!(count.load(Ordering::SeqCst), 2);
    program.build().unwrap();
}

#[test]
fn assembly_carries_a_subset_of_the_programs_capabilities() {
    let mut program = Program::builder();
    // Another Component's capability, outside the Assembly.
    program.add_component::<Holds<Source<StreamDescriptor<u64>>>>(());
    let count = Arc::new(AtomicUsize::new(0));
    let installed = program.add_assembly::<Library>(count.clone());
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let _: &(Effect<Read>,) = &installed.io;
    let reader = installed.exports().clone();
    drop(installed); // The Program itself carries all binding obligations.
    let program = program.build().unwrap();
    assert_eq!(program.manifest().capabilities().len(), 2);
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
        .build()
        .unwrap();
    runtime.send(&reader, None).unwrap();
    runtime.run_until_idle().unwrap();
    let pending = runtime.next_effect::<Read>().unwrap();
    runtime
        .complete(pending, EffectOutcome::Succeeded(42))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(runtime.state(&reader).unwrap(), &[42]);
    runtime.cancel().unwrap();
}
