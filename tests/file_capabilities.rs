#![cfg(feature = "fs")]
//! ADR-0022: one file resource, typed permissions and operation results.
use bytes::Bytes;
use samara::prelude::*;
use std::{
    convert::Infallible,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

type Needs = (File<ReadAppend>, Effect<Report>);
type ProgramNeeds = (File<ReadAppend>, File<ReadAppend>, Effect<Report>);
type Fixture = (
    Program<ProgramNeeds>,
    [ComponentRef<Writer>; 2],
    [FileCapability<ReadAppend>; 2],
);
struct Report(String);
impl EffectDescriptor for Report {
    type Output = ();
    type Error = Infallible;
}
struct Reporter(tokio::sync::mpsc::UnboundedSender<String>);
impl EffectDriver<Report> for Reporter {
    fn execute(&self, Report(text): Report) -> BoxFuture<Result<(), Infallible>> {
        let sender = self.0.clone();
        Box::pin(async move {
            sender.send(text).unwrap();
            Ok(())
        })
    }
}
struct Writer;
enum Message {
    Start,
    Appended(EffectOutcome<(), FsError>),
    Read(EffectOutcome<Bytes, FsError>),
}
impl Component for Writer {
    type Requires = Needs;
    type Config = &'static str;
    type Model = Bytes;
    type Message = Message;
    fn init(_: &Self::Config, _: &Capabilities<Needs>) -> Init<Bytes, Message> {
        Init::default()
    }
    fn update(
        text: &Self::Config,
        (file, report): &Capabilities<Needs>,
        model: &mut Bytes,
        message: Message,
    ) -> Command<Message> {
        match message {
            Message::Start => file.append_line(*text).into_command_with(Message::Appended),
            Message::Appended(EffectOutcome::Succeeded(())) => {
                file.read().into_command_with(Message::Read)
            }
            Message::Read(EffectOutcome::Succeeded(contents)) => {
                *model = contents;
                Command::effect_discarding_outcome(
                    report,
                    Report(String::from_utf8(model.to_vec()).unwrap()),
                )
            }
            Message::Appended(EffectOutcome::Failed(_))
            | Message::Read(EffectOutcome::Failed(_)) => Command::none(),
        }
    }
}
fn fixture() -> Fixture {
    let (mut builder, (left, right, report)) = Program::builder::<ProgramNeeds>();
    let first = builder
        .component::<Writer>("first")
        .with_capabilities((left.clone(), report.clone()))
        .finish()
        .unwrap();
    let second = builder
        .component::<Writer>("second")
        .with_capabilities((right.clone(), report))
        .finish()
        .unwrap();
    (builder.build().unwrap(), [first, second], [left, right])
}

#[test]
fn file_operations_are_inert_and_preserve_their_typed_results() {
    let (_, file) = Program::builder::<File<ReadAppend>>();
    let append: Command<EffectOutcome<(), FsError>> = file.append_line("hello").into_command();
    assert_eq!(
        append.effect_intent::<FileAppend>().unwrap().as_bytes(),
        b"hello\n"
    );
    let read: Command<EffectOutcome<Bytes, FsError>> = file.read().into_command();
    assert!(read.effect_intent::<FileRead>().is_some());
    let append = file.append_text("raw").discard_outcome::<()>();
    assert_eq!(
        append.effect_intent::<FileAppend>().unwrap().as_bytes(),
        b"raw"
    );
}

#[test]
fn write_authority_constructs_typed_replacements_and_narrows_to_existing_permissions() {
    let (mut builder, (file, report)) = Program::builder::<(File<ReadWrite>, Effect<Report>)>();
    let replace: Command<EffectOutcome<(), FsError>> =
        file.replace_text("new contents").into_command();
    assert_eq!(
        replace.effect_intent::<FileReplace>().unwrap().as_str(),
        Some("new contents")
    );
    let binary = file
        .write_only()
        .replace_bytes(Bytes::from_static(&[0, 255]))
        .discard_outcome::<()>();
    assert_eq!(
        binary.effect_intent::<FileReplace>().unwrap().as_bytes(),
        &[0, 255]
    );
    let append = file
        .write_only()
        .append_text("tail")
        .discard_outcome::<()>();
    assert_eq!(
        append.effect_intent::<FileAppend>().unwrap().as_str(),
        Some("tail")
    );
    // Both existing component permission sets accept narrower views of this resource.
    let writer = builder
        .component::<Writer>("append")
        .with_capabilities((file.read_append(), report))
        .finish()
        .unwrap();
    builder
        .component::<Reader>(())
        .with_capabilities(file.read_only())
        .finish()
        .unwrap();
    let mut runtime = ControlledRuntime::builder(builder.build().unwrap())
        .control_all()
        .build()
        .unwrap();
    runtime.send(&writer, Message::Start).unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.next_file_read(&file).is_ok());
    assert_eq!(
        runtime.next_file_append(&file).unwrap().intent.as_str(),
        Some("append\n")
    );
    assert!(runtime.cancel().unwrap().is_clean());
}

struct Replacer;
impl Component for Replacer {
    type Requires = File<WriteOnly>;
    type Config = &'static str;
    type Message = EffectOutcome<(), FsError>;
    type Model = Option<Self::Message>;
    fn init(
        text: &Self::Config,
        file: &Capabilities<Self::Requires>,
    ) -> Init<Self::Model, Self::Message> {
        Init::new(None).with_command(file.replace_text(*text).into_command())
    }
    fn update(
        _: &Self::Config,
        _: &Capabilities<Self::Requires>,
        model: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        *model = Some(message);
        Command::none()
    }
}

#[test]
fn replacements_select_the_exact_resource_and_deliver_typed_outcomes() {
    for outcome in [
        EffectOutcome::Succeeded(()),
        EffectOutcome::Failed(EffectFailure::Error(FsError::new(
            FsOperation::Replace,
            "fixture",
            FsErrorKind::PermissionDenied,
            "read-only fixture",
        ))),
        EffectOutcome::Failed(EffectFailure::Cancelled(CancelReason::Superseded)),
    ] {
        let expected = outcome.clone();
        let (mut builder, (left, right)) = Program::builder::<(File<ReadWrite>, File<WriteOnly>)>();
        let first = builder
            .component::<Replacer>("first")
            .with_capabilities(left.write_only())
            .finish()
            .unwrap();
        let second = builder
            .component::<Replacer>("second")
            .with_capabilities(right.clone())
            .finish()
            .unwrap();
        let mut runtime = ControlledRuntime::builder(builder.build().unwrap())
            .control_all()
            .build()
            .unwrap();
        runtime.run_until_idle().unwrap();
        let (_, foreign) = Program::builder::<File<WriteOnly>>();
        assert!(runtime.next_file_replace(&foreign).is_err());
        let second_replace = runtime.next_file_replace(&right).unwrap();
        assert_eq!(second_replace.intent.as_str(), Some("second"));
        let first_replace = runtime.next_file_replace(&left.write_only()).unwrap();
        assert_eq!(first_replace.intent.as_str(), Some("first"));
        runtime.complete(second_replace, outcome).unwrap();
        runtime
            .complete(first_replace, EffectOutcome::Succeeded(()))
            .unwrap();
        runtime.run_until_idle().unwrap();
        assert!(matches!(
            runtime.state(&first).unwrap(),
            Some(EffectOutcome::Succeeded(()))
        ));
        assert_eq!(runtime.state(&second).unwrap().as_ref().unwrap(), &expected);
        assert_eq!(runtime.pending_work(), PendingWork::default());
        assert!(runtime.cancel().unwrap().is_clean());
    }
}

#[tokio::test]
async fn live_write_permissions_replace_existing_contents_at_each_bound_path() {
    let root = TempRoot::new();
    std::fs::create_dir_all(&root.0).unwrap();
    let first_path = root.0.join("existing.log");
    let second_path = root.0.join("nested/new.log");
    std::fs::write(&first_path, "old contents must be entirely replaced").unwrap();
    let (mut builder, (left, right)) = Program::builder::<(File<ReadWrite>, File<WriteOnly>)>();
    builder
        .component::<Replacer>("first")
        .with_capabilities(left.write_only())
        .finish()
        .unwrap();
    builder
        .component::<Replacer>("second")
        .with_capabilities(right)
        .finish()
        .unwrap();
    let runtime = Runtime::builder(builder.build().unwrap())
        .with_drivers((FileDriver::new(&first_path), FileDriver::new(&second_path)))
        .build()
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&first_path).unwrap(),
        "old contents must be entirely replaced"
    );
    assert!(!second_path.exists(), "binding must not create the target");
    let task = runtime.spawn();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), task.shutdown(Shutdown::Drain))
            .await
            .unwrap()
            .unwrap()
            .is_clean()
    );
    assert_eq!(std::fs::read(&first_path).unwrap(), b"first");
    assert_eq!(std::fs::read(&second_path).unwrap(), b"second");
}

#[test]
fn controlled_effects_select_the_exact_file_and_keep_narrowed_identity() {
    let (program, components, files) = fixture();
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
        .build()
        .unwrap();
    for component in &components {
        runtime.send(component, Message::Start).unwrap();
    }
    runtime.run_until_idle().unwrap();
    // Take the later operation first by resource, not the type-wide issue order.
    let second = runtime.next_file_append(&files[1].append_only()).unwrap();
    assert_eq!(second.intent.as_bytes(), b"second\n");
    let first = runtime.next_file_append(&files[0]).unwrap();
    assert_eq!(first.intent.as_bytes(), b"first\n");
    runtime
        .complete(second, EffectOutcome::Succeeded(()))
        .unwrap();
    runtime
        .complete(first, EffectOutcome::Succeeded(()))
        .unwrap();
    runtime.run_until_idle().unwrap();
    let first_read = runtime.next_file_read(&files[0].read_only()).unwrap();
    let second_read = runtime.next_file_read(&files[1]).unwrap();
    runtime
        .complete(
            first_read,
            EffectOutcome::Succeeded(Bytes::from_static(b"left")),
        )
        .unwrap();
    runtime
        .complete(
            second_read,
            EffectOutcome::Succeeded(Bytes::from_static(b"right")),
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(runtime.state(&components[0]).unwrap().as_ref(), b"left");
    assert_eq!(runtime.state(&components[1]).unwrap().as_ref(), b"right");
    assert!(runtime.cancel().unwrap().is_clean());
}

#[test]
fn foreign_file_selector_does_not_consume_another_files_operation() {
    let (program, components, files) = fixture();
    let (_, foreign) = Program::builder::<File<ReadAppend>>();
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
        .build()
        .unwrap();
    runtime.send(&components[0], Message::Start).unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.next_file_append(&foreign).is_err());
    let pending = runtime.next_file_append(&files[0]).unwrap();
    runtime
        .complete(
            pending,
            EffectOutcome::Failed(EffectFailure::Cancelled(CancelReason::Superseded)),
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.state(&components[0]).unwrap().is_empty());
    assert!(runtime.next_file_read(&files[0]).is_err());
    assert!(runtime.cancel().unwrap().is_clean());
}

#[test]
fn foreign_file_is_rejected_during_component_registration() {
    let (_, foreign) = Program::builder::<File<ReadAppend>>();
    let (mut builder, (_, _, report)) = Program::builder::<ProgramNeeds>();
    assert!(
        builder
            .component::<Writer>("foreign")
            .with_capabilities((foreign, report))
            .finish()
            .is_err()
    );
}

struct Reader;
impl Component for Reader {
    type Requires = File<ReadOnly>;
    type Config = ();
    type Model = Bytes;
    type Message = EffectOutcome<Bytes, FsError>;
    fn init(_: &(), file: &Capabilities<Self::Requires>) -> Init<Bytes, Self::Message> {
        Init::new(Bytes::new()).with_command(file.read().into_command())
    }
    fn update(
        _: &(),
        _: &Capabilities<Self::Requires>,
        model: &mut Bytes,
        message: Self::Message,
    ) -> Command<Self::Message> {
        if let EffectOutcome::Succeeded(bytes) = message {
            *model = bytes;
        }
        Command::none()
    }
}

#[test]
fn narrowed_capability_supplies_a_read_only_component_without_another_binding() {
    let (mut builder, file) = Program::builder::<File<ReadAppend>>();
    let reader = builder
        .component::<Reader>(())
        .with_capabilities(file.read_only())
        .finish()
        .unwrap();
    let mut runtime = ControlledRuntime::builder(builder.build().unwrap())
        .control_all()
        .build()
        .unwrap();
    runtime.run_until_idle().unwrap();
    let pending = runtime.next_file_read(&file).unwrap();
    runtime
        .complete(
            pending,
            EffectOutcome::Succeeded(Bytes::from_static(b"shared")),
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(runtime.state(&reader).unwrap().as_ref(), b"shared");
    assert!(runtime.cancel().unwrap().is_clean());
}

#[test]
fn controlled_file_traces_repeat_for_identical_inputs() {
    fn trace() -> Vec<TraceRecord> {
        let (program, components, files) = fixture();
        let mut runtime = ControlledRuntime::builder(program)
            .control_all()
            .build()
            .unwrap();
        for component in &components {
            runtime.send(component, Message::Start).unwrap();
        }
        runtime.run_until_idle().unwrap();
        for file in files.iter().rev() {
            let pending = runtime.next_file_append(file).unwrap();
            runtime
                .complete(pending, EffectOutcome::Succeeded(()))
                .unwrap();
        }
        runtime.run_until_idle().unwrap();
        for file in &files {
            let pending = runtime.next_file_read(file).unwrap();
            runtime
                .complete(
                    pending,
                    EffectOutcome::Succeeded(Bytes::from_static(b"controlled")),
                )
                .unwrap();
        }
        runtime.run_until_idle().unwrap();
        let trace = runtime.trace().to_vec();
        assert!(runtime.cancel().unwrap().is_clean());
        trace
    }
    assert_eq!(trace(), trace());
}

struct HiddenFile;
impl Component for HiddenFile {
    type Requires = ();
    type Config = (FileCapability<ReadOnly>, bool);
    type Model = ();
    type Message = bool;
    fn init(config: &Self::Config, _: &()) -> Init<(), bool> {
        Init::new(()).with_command(if config.1 {
            config.0.read().into_command_with(|_| false)
        } else {
            Command::none()
        })
    }
    fn update(config: &Self::Config, _: &(), _: &mut (), start: bool) -> Command<bool> {
        if start {
            config.0.read().into_command_with(|_| false)
        } else {
            Command::none()
        }
    }
}

#[test]
fn hidden_foreign_file_is_rejected_at_startup_or_before_controlled_execution() {
    for initial in [true, false] {
        let (_, foreign) = Program::builder::<File<ReadOnly>>();
        let (mut builder, _) = Program::builder::<File<ReadOnly>>();
        let target = builder
            .component::<HiddenFile>((foreign, initial))
            .finish()
            .unwrap();
        let runtime = ControlledRuntime::builder(builder.build().unwrap())
            .control_all()
            .build();
        if initial {
            assert!(runtime.is_err());
        } else {
            let mut runtime = runtime.unwrap();
            runtime.send(&target, true).unwrap();
            assert!(
                runtime
                    .run_until_idle()
                    .unwrap_err()
                    .to_string()
                    .contains("another Program")
            );
        }
    }
}

#[tokio::test]
async fn hidden_foreign_file_is_rejected_before_live_execution() {
    for initial in [true, false] {
        let (_, foreign) = Program::builder::<File<ReadOnly>>();
        let (mut builder, _) = Program::builder::<File<ReadOnly>>();
        let target = builder
            .component::<HiddenFile>((foreign, initial))
            .finish()
            .unwrap();
        let root = TempRoot::new();
        let runtime = Runtime::builder(builder.build().unwrap())
            .with_drivers(FileDriver::new(root.0.join("unread.log")))
            .build();
        if initial {
            assert!(runtime.is_err());
        } else {
            let runtime = runtime.unwrap();
            let handle = runtime.handle(&target).unwrap();
            let mut task = runtime.spawn();
            handle.send(true).await.unwrap();
            let error = tokio::time::timeout(Duration::from_secs(2), task.run_forever())
                .await
                .expect("foreign effect must fault promptly")
                .unwrap_err();
            assert!(error.to_string().contains("another Program"));
        }
    }
}

struct TempRoot(PathBuf);
impl TempRoot {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(std::env::temp_dir().join(format!(
            "samara-file-capability-{}-{}",
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
async fn same_type_files_bind_independently_and_read_their_own_appends() {
    let root = TempRoot::new();
    let (program, components, _) = fixture();
    let (sender, mut reports) = tokio::sync::mpsc::unbounded_channel();
    let runtime = Runtime::builder(program)
        .with_drivers((
            FileDriver::new(root.0.join("left/events.log")),
            FileDriver::new(root.0.join("right/events.log")),
            Reporter(sender),
        ))
        .build()
        .unwrap();
    assert!(!root.0.exists(), "binding a file must not perform I/O");
    let handles = components
        .each_ref()
        .map(|component| runtime.handle(component).unwrap());
    let runtime = runtime.spawn();
    for handle in handles {
        handle.send(Message::Start).await.unwrap();
    }
    let mut seen = tokio::time::timeout(Duration::from_secs(2), async {
        vec![reports.recv().await.unwrap(), reports.recv().await.unwrap()]
    })
    .await
    .expect("both files must complete their append/read sequence");
    seen.sort();
    assert_eq!(seen, ["first\n", "second\n"]);
    assert!(runtime.shutdown(Shutdown::Drain).await.unwrap().is_clean());
    assert_eq!(
        std::fs::read(root.0.join("left/events.log")).unwrap(),
        b"first\n"
    );
    assert_eq!(
        std::fs::read(root.0.join("right/events.log")).unwrap(),
        b"second\n"
    );
}
