//! ADR-0019 acceptance: the Program owns its typed binding obligations.
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
    type Config = Arc<AtomicUsize>;
    type Requires = (Effect<Read>,);
    type Model = Vec<u64>;
    type Message = Option<u64>;
    fn init(
        count: &Self::Config,
        _: &Capabilities<Self::Requires>,
    ) -> Init<Self::Model, Self::Message> {
        count.fetch_add(1, Ordering::SeqCst);
        Init::default()
    }
    fn update(
        _: &Self::Config,
        (read,): &Capabilities<Self::Requires>,
        model: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        if let Some(value) = message {
            model.push(value);
            Command::none()
        } else {
            Command::effect_with(read, Read, |outcome| match outcome {
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

#[test]
fn shared_capability_reaches_callbacks_and_delayed_effects() {
    let (mut program, (read,)) = Program::builder::<(Effect<Read>,)>();
    let count = Arc::new(AtomicUsize::new(0));
    let first = program
        .component::<Reader>(count.clone())
        .name("first")
        .with_capabilities((read.clone(),))
        .finish()
        .unwrap();
    let second = program
        .component::<Reader>(count.clone())
        .name("second")
        .with_capabilities((read,))
        .finish()
        .unwrap();
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

#[test]
fn foreign_registration_fails_before_init_and_leaves_no_component() {
    let (_, foreign) = Program::builder::<(Effect<Read>,)>();
    let (mut program, own) = Program::builder::<(Effect<Read>,)>();
    let count = Arc::new(AtomicUsize::new(0));
    assert!(
        program
            .component::<Reader>(count.clone())
            .name("reader")
            .with_capabilities(foreign)
            .finish()
            .is_err()
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
    program
        .component::<Reader>(count.clone())
        .name("reader")
        .with_capabilities(own)
        .finish()
        .unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    program.build().unwrap();
}

#[tokio::test]
async fn live_tuple_binds_one_driver_for_shared_capabilities() {
    let (mut program, (read,)) = Program::builder::<(Effect<Read>,)>();
    let count = Arc::new(AtomicUsize::new(0));
    let first = program
        .component::<Reader>(count.clone())
        .name("first")
        .with_capabilities((read.clone(),))
        .finish()
        .unwrap();
    program
        .component::<Reader>(count)
        .name("second")
        .with_capabilities((read,))
        .finish()
        .unwrap();
    let runtime = Runtime::builder(program.build().unwrap())
        .with_drivers((ReadDriver,))
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
    let (program, _) = Program::builder::<Inputs>();
    let (_, first) = tokio::sync::mpsc::channel(1);
    let (_, second) = tokio::sync::mpsc::channel(1);
    let tokio = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _guard = tokio.enter();
    Runtime::builder(program.build().unwrap())
        .with_drivers((MpscReceiver::new(first), MpscReceiver::new(second)))
        .build()
        .unwrap();
    let (program, _) = Program::builder::<Inputs>();
    ControlledRuntime::builder(program.build().unwrap())
        .control_all()
        .build()
        .unwrap();
}

struct Library;
impl Assembly for Library {
    type Config = Arc<AtomicUsize>;
    type Accepts = Option<u64>;
    type Exports = ComponentRef<Reader>;
    type Requires = (Effect<Read>,);
    fn install<R: Requirements>(
        program: &mut ProgramBuilder<R>,
        count: Self::Config,
        caps: Capabilities<Self::Requires>,
    ) -> Result<(Connector<Self::Accepts>, Self::Exports), ProgramBuildError> {
        let reader = program
            .component::<Reader>(count)
            .name("library")
            .with_capabilities(caps)
            .finish()?;
        Ok((reader.connector(), reader))
    }
}

#[test]
fn repeated_assembly_installations_need_no_component_name_coordination() {
    let (mut program, caps) = Program::builder::<(Effect<Read>,)>();
    let count = Arc::new(AtomicUsize::new(0));
    let first = program
        .install::<Library>(count.clone())
        .with_capabilities(caps.clone())
        .finish()
        .unwrap();
    let second = program
        .install::<Library>(count.clone())
        .with_capabilities(caps)
        .finish()
        .unwrap();
    assert_eq!(
        first.connector().id().name(),
        second.connector().id().name()
    );
    assert_ne!(first.connector().id(), second.connector().id());
    assert_eq!(count.load(Ordering::SeqCst), 2);
    program.build().unwrap();
}

#[test]
fn assembly_forwards_a_subset_of_nested_program_requirements() {
    let (mut program, (reader_caps, _input)) =
        Program::builder::<((Effect<Read>,), Source<StreamDescriptor<u64>>)>();
    let count = Arc::new(AtomicUsize::new(0));
    let installed = program
        .install::<Library>(count.clone())
        .with_capabilities(reader_caps)
        .finish()
        .unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let reader = installed.exports().clone();
    drop(installed); // The Program itself carries all binding obligations.
    let mut runtime = ControlledRuntime::builder(program.build().unwrap())
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

#[test]
fn foreign_assembly_capabilities_fail_before_installation() {
    let (_, foreign) = Program::builder::<(Effect<Read>,)>();
    let (mut program, own) = Program::builder::<(Effect<Read>,)>();
    let count = Arc::new(AtomicUsize::new(0));
    assert!(
        program
            .install::<Library>(count.clone())
            .with_capabilities(foreign)
            .finish()
            .is_err()
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
    program
        .install::<Library>(count)
        .with_capabilities(own)
        .finish()
        .unwrap();
    program.build().unwrap();
}
