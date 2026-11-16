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
/// The Program, its two Writers, each Writer's own file, and each Writer's
/// own report effect.
type Fixture = (
    Program,
    [ComponentRef<Writer>; 2],
    [File<ReadAppend>; 2],
    [Effect<Report>; 2],
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
    type Io = Needs;
    type Connections = ();
    type Configuration = &'static str;
    type Model = Bytes;
    type Message = Message;
    fn init(_: &Context<Self>) -> Init<Bytes, Message> {
        Init::default()
    }
    fn update(cx: &Context<Self>, model: &mut Bytes, message: Message) -> Command<Message> {
        let (file, report) = &cx.io;
        match message {
            Message::Start => File::append_line(file, cx.config).producing(Message::Appended),
            Message::Appended(EffectOutcome::Succeeded(())) => {
                File::read(file).producing(Message::Read)
            }
            Message::Read(EffectOutcome::Succeeded(contents)) => {
                *model = contents;
                Effect::perform(report, Report(String::from_utf8(model.to_vec()).unwrap()))
                    .discard_outcome()
            }
            Message::Appended(EffectOutcome::Failed(_))
            | Message::Read(EffectOutcome::Failed(_)) => Command::none(),
        }
    }
}
fn fixture() -> Fixture {
    let mut builder = Program::builder();
    let first = builder
        .configure_component::<Writer>()
        .configuration("first")
        .add(());
    let second = builder
        .configure_component::<Writer>()
        .configuration("second")
        .add(());
    let files = [first.io.0.clone(), second.io.0.clone()];
    let reports = [first.io.1.clone(), second.io.1.clone()];
    (
        builder.build().unwrap(),
        [first.into(), second.into()],
        files,
        reports,
    )
}

#[test]
fn file_operations_are_inert_and_preserve_their_typed_results() {
    let file = <File<ReadAppend>>::detached();
    let append: Command<EffectOutcome<(), FsError>> =
        File::append_line(&file, "hello").into_command();
    assert_eq!(
        append.effect_intent::<FileAppend>().unwrap().as_bytes(),
        b"hello\n"
    );
    let read: Command<EffectOutcome<Bytes, FsError>> = File::read(&file).into_command();
    assert!(read.effect_intent::<FileRead>().is_some());
    let append = File::append_text(&file, "raw").discard_outcome::<()>();
    assert_eq!(
        append.effect_intent::<FileAppend>().unwrap().as_bytes(),
        b"raw"
    );
}

#[test]
fn write_authority_constructs_typed_replacements() {
    let file = <File<ReadWrite>>::detached();
    let replace: Command<EffectOutcome<(), FsError>> =
        File::replace_text(&file, "new contents").into_command();
    assert_eq!(
        replace.effect_intent::<FileReplace>().unwrap().as_str(),
        Some("new contents")
    );
    let write_only = <File<WriteOnly>>::detached();
    let binary =
        File::replace_bytes(&write_only, Bytes::from_static(&[0, 255])).discard_outcome::<()>();
    assert_eq!(
        binary.effect_intent::<FileReplace>().unwrap().as_bytes(),
        &[0, 255]
    );
    let append = File::append_text(&write_only, "tail").discard_outcome::<()>();
    assert_eq!(
        append.effect_intent::<FileAppend>().unwrap().as_str(),
        Some("tail")
    );
}

struct Replacer;
impl Component for Replacer {
    type Io = File<WriteOnly>;
    type Connections = ();
    type Configuration = &'static str;
    type Message = EffectOutcome<(), FsError>;
    type Model = Option<Self::Message>;
    fn init(cx: &Context<Self>) -> Init<Self::Model, Self::Message> {
        Init::new(None).with_command(File::replace_text(&cx.io, cx.config).into_command())
    }
    fn update(
        _: &Context<Self>,
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
        EffectOutcome::Failed(FsError::new(
            FsOperation::Replace,
            "fixture",
            FsErrorKind::PermissionDenied,
            "read-only fixture",
        )),
    ] {
        let expected = outcome.clone();
        let mut builder = Program::builder();
        let first = builder
            .configure_component::<Replacer>()
            .configuration("first")
            .add(());
        let second = builder
            .configure_component::<Replacer>()
            .configuration("second")
            .add(());
        let mut runtime = ControlledRuntime::builder(builder.build().unwrap())
            .control_all()
            .build()
            .unwrap();
        runtime.run_until_idle().unwrap();
        let foreign = <File<WriteOnly>>::detached();
        assert!(runtime.next_file_replace(&foreign).is_err());
        let second_replace = runtime.next_file_replace(&second.io).unwrap();
        assert_eq!(second_replace.intent.as_str(), Some("second"));
        let first_replace = runtime.next_file_replace(&first.io).unwrap();
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
    let mut builder = Program::builder();
    let first = builder
        .configure_component::<Replacer>()
        .configuration("first")
        .add(());
    let second = builder
        .configure_component::<Replacer>()
        .configuration("second")
        .add(());
    let runtime = Runtime::builder(builder.build().unwrap())
        .supply(&first.io, FileDriver::new(&first_path))
        .supply(&second.io, FileDriver::new(&second_path))
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
fn controlled_effects_select_the_exact_file() {
    let (program, components, files, _) = fixture();
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
        .build()
        .unwrap();
    for component in &components {
        runtime.send(component, Message::Start).unwrap();
    }
    runtime.run_until_idle().unwrap();
    // Take the later operation first by resource, not the type-wide issue order.
    let second = runtime.next_file_append(&files[1]).unwrap();
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
    let first_read = runtime.next_file_read(&files[0]).unwrap();
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
    let (program, components, files, _) = fixture();
    let foreign = <File<ReadAppend>>::detached();
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
            EffectOutcome::Failed(FsError::new(
                FsOperation::Append,
                "fixture",
                FsErrorKind::PermissionDenied,
                "read-only fixture",
            )),
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.state(&components[0]).unwrap().is_empty());
    assert!(runtime.next_file_read(&files[0]).is_err());
    assert!(runtime.cancel().unwrap().is_clean());
}

#[test]
fn controlled_file_traces_repeat_for_identical_inputs() {
    fn trace() -> Vec<TraceRecord> {
        let (program, components, files, _) = fixture();
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

/// Owns a file it never uses, and reads one smuggled in through Config.
struct HiddenFile;
impl Component for HiddenFile {
    type Io = File<ReadOnly>;
    type Connections = ();
    type Configuration = (File<ReadOnly>, bool);
    type Model = ();
    type Message = bool;
    fn init(cx: &Context<Self>) -> Init<(), bool> {
        Init::new(()).with_command(if cx.config.1 {
            File::read(&cx.config.0).producing(|_| false)
        } else {
            Command::none()
        })
    }
    fn update(cx: &Context<Self>, _: &mut (), start: bool) -> Command<bool> {
        if start {
            File::read(&cx.config.0).producing(|_| false)
        } else {
            Command::none()
        }
    }
}

#[test]
fn hidden_foreign_file_is_rejected_at_startup_or_before_controlled_execution() {
    for initial in [true, false] {
        let foreign = <File<ReadOnly>>::detached();
        let mut builder = Program::builder();
        let target = builder
            .configure_component::<HiddenFile>()
            .configuration((foreign, initial))
            .add(());
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
        let foreign = <File<ReadOnly>>::detached();
        let mut builder = Program::builder();
        let target = builder
            .configure_component::<HiddenFile>()
            .configuration((foreign, initial))
            .add(());
        let root = TempRoot::new();
        let runtime = Runtime::builder(builder.build().unwrap())
            .supply(&target.io, FileDriver::new(root.0.join("unread.log")))
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
    let (program, components, files, report_effects) = fixture();
    let (sender, mut reports) = tokio::sync::mpsc::unbounded_channel();
    let runtime = Runtime::builder(program)
        .supply(&files[0], FileDriver::new(root.0.join("left/events.log")))
        .supply(&files[1], FileDriver::new(root.0.join("right/events.log")))
        .supply(&report_effects[0], Reporter(sender.clone()))
        .supply(&report_effects[1], Reporter(sender))
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
