#![cfg(feature = "fs")]

use std::convert::Infallible;
use std::path::{Path, PathBuf};

use samara::prelude::*;

/// Reports a line to the test through a channel, so live tests can observe a
/// Component without reading its Model.
#[derive(Clone, Debug)]
struct Report(String);

impl EffectDescriptor for Report {
    type Output = ();
    type Error = Infallible;
}

struct Reporter(tokio::sync::mpsc::UnboundedSender<String>);

impl EffectDriver<Report> for Reporter {
    fn execute(&self, Report(line): Report) -> BoxFuture<Result<(), Infallible>> {
        let sender = self.0.clone();
        Box::pin(async move {
            let _ = sender.send(line);
            Ok(())
        })
    }
}

const FILE: &str = "notes/journal.log";

/// Walks one file through append, append, read, replace, read, remove, read,
/// reporting what it sees. Each step waits for the previous outcome, which is
/// the only ordering the effects promise today.
struct Walker;

struct WalkerConfig {
    append: EffectCapability<AppendToFile>,
    replace: EffectCapability<ReplaceFile>,
    read: EffectCapability<ReadFile>,
    remove: EffectCapability<RemoveFile>,
    report: EffectCapability<Report>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct WalkerModel {
    step: usize,
    seen: Vec<String>,
    failures: Vec<(FsOperation, FsErrorKind, PathBuf)>,
}

enum WalkerMessage {
    Start,
    Escape,
    Wrote(EffectOutcome<(), FsError>),
    Read(EffectOutcome<bytes::Bytes, FsError>),
    Reported,
}

impl Component for Walker {
    type Config = WalkerConfig;
    type Model = WalkerModel;
    type Message = WalkerMessage;

    fn init(_: &WalkerConfig) -> Init<WalkerModel, WalkerMessage> {
        Init::new(WalkerModel::default())
    }

    fn update(
        config: &WalkerConfig,
        model: &mut WalkerModel,
        message: WalkerMessage,
    ) -> Command<WalkerMessage> {
        let next = |model: &mut WalkerModel| -> Command<WalkerMessage> {
            model.step += 1;
            match model.step {
                1 => Command::effect_with(
                    &config.append,
                    AppendToFile::line(FILE, "first"),
                    WalkerMessage::Wrote,
                ),
                2 => Command::effect_with(
                    &config.append,
                    AppendToFile::line(FILE, "second"),
                    WalkerMessage::Wrote,
                ),
                3 | 5 | 7 => {
                    Command::effect_with(&config.read, ReadFile::new(FILE), WalkerMessage::Read)
                }
                4 => Command::effect_with(
                    &config.replace,
                    ReplaceFile::text(FILE, "replaced"),
                    WalkerMessage::Wrote,
                ),
                6 => Command::effect_with(
                    &config.remove,
                    RemoveFile::new(FILE),
                    WalkerMessage::Wrote,
                ),
                _ => Command::effect_with(&config.report, Report("done".into()), |_| {
                    WalkerMessage::Reported
                }),
            }
        };
        match message {
            WalkerMessage::Start => next(model),
            WalkerMessage::Escape => Command::effect_with(
                &config.append,
                AppendToFile::line("../escape.log", "nope"),
                WalkerMessage::Wrote,
            ),
            WalkerMessage::Wrote(EffectOutcome::Succeeded(())) => next(model),
            WalkerMessage::Read(EffectOutcome::Succeeded(bytes)) => {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                model.seen.push(text.clone());
                Command::batch([
                    Command::effect_with(&config.report, Report(text), |_| WalkerMessage::Reported),
                    next(model),
                ])
            }
            WalkerMessage::Wrote(EffectOutcome::Failed(error))
            | WalkerMessage::Read(EffectOutcome::Failed(error)) => {
                model
                    .failures
                    .push((error.operation(), error.kind(), error.path().to_path_buf()));
                let line = format!("{:?} {:?}", error.operation(), error.kind());
                Command::batch([
                    Command::effect_with(&config.report, Report(line), |_| WalkerMessage::Reported),
                    if model.step == 7 {
                        next(model)
                    } else {
                        Command::none()
                    },
                ])
            }
            WalkerMessage::Wrote(EffectOutcome::Cancelled(_))
            | WalkerMessage::Read(EffectOutcome::Cancelled(_))
            | WalkerMessage::Reported => Command::none(),
        }
    }
}

fn walker_program() -> (Program, ComponentRef<Walker>) {
    let mut builder = Program::builder();
    let config = WalkerConfig {
        append: builder.effect::<AppendToFile>(),
        replace: builder.effect::<ReplaceFile>(),
        read: builder.effect::<ReadFile>(),
        remove: builder.effect::<RemoveFile>(),
        report: builder.effect::<Report>(),
    };
    let walker = builder.component::<Walker>(ComponentId::new("walker"), config);
    (builder.build().expect("valid walker program"), walker)
}

fn unique_root(label: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("samara-fs-{label}-{}-{nonce}", std::process::id()))
}

#[test]
fn descriptors_carry_relative_paths_and_bytes_without_io() {
    let append = AppendToFile::line("a/b.log", "hi");
    assert_eq!(append.path(), Path::new("a/b.log"));
    assert_eq!(append.as_bytes(), b"hi\n");
    assert_eq!(append.as_str(), Some("hi\n"));
    assert_eq!(AppendToFile::text("x", "raw").as_bytes(), b"raw");
    assert_eq!(AppendToFile::bytes("x", vec![0xff]).as_str(), None);
    assert_eq!(ReplaceFile::text("x", "{}").as_str(), Some("{}"));
    assert_eq!(ReadFile::new("x").path(), Path::new("x"));
    assert_eq!(RemoveFile::new("x").path(), Path::new("x"));

    let error = FsError::new(FsOperation::Remove, "x", FsErrorKind::NotFound, "gone");
    assert_eq!(error.message(), "gone");
    assert_eq!(error.path(), Path::new("x"));
}

#[test]
fn filesystem_effects_are_ordinary_controlled_boundaries() {
    let (program, walker) = walker_program();
    let mut runtime = ControlledRuntime::builder(program)
        .control_effect::<AppendToFile>()
        .control_effect::<ReplaceFile>()
        .control_effect::<ReadFile>()
        .control_effect::<RemoveFile>()
        .control_effect::<Report>()
        .build()
        .expect("valid controlled bindings");

    runtime.send(&walker, WalkerMessage::Start).expect("start");
    runtime.run_until_idle().expect("first append issued");

    // The test sees the intent by relative path; no root exists here.
    let first = runtime.next_effect::<AppendToFile>().expect("first append");
    assert_eq!(first.intent.path(), Path::new(FILE));
    assert_eq!(first.intent.as_str(), Some("first\n"));
    runtime
        .complete(first, EffectOutcome::Succeeded(()))
        .expect("first done");
    runtime.run_until_idle().expect("second append issued");

    let second = runtime
        .next_effect::<AppendToFile>()
        .expect("second append");
    assert_eq!(second.intent.as_str(), Some("second\n"));
    runtime
        .complete(
            second,
            EffectOutcome::Failed(FsError::new(
                FsOperation::Append,
                FILE,
                FsErrorKind::PermissionDenied,
                "read-only volume",
            )),
        )
        .expect("failure accepted");
    runtime.run_until_idle().expect("failure mapped");

    let report = runtime.next_effect::<Report>().expect("failure reported");
    assert_eq!(report.intent.0, "Append PermissionDenied");
    assert!(runtime.next_effect::<AppendToFile>().is_err());

    let model = runtime.state(&walker).expect("walker model");
    assert_eq!(model.step, 2);
    assert_eq!(
        model.failures,
        vec![(
            FsOperation::Append,
            FsErrorKind::PermissionDenied,
            PathBuf::from(FILE)
        )]
    );
    runtime.cancel().expect("clean close");
}

#[tokio::test]
async fn bound_driver_appends_reads_replaces_and_removes_under_the_root() {
    let root = unique_root("walk");
    let (program, walker) = walker_program();
    let (sender, mut reports) = tokio::sync::mpsc::unbounded_channel();
    let runtime = Runtime::builder(program)
        .bind_fs(&root)
        .bind_effect::<Report, _>(Reporter(sender))
        .build()
        .expect("valid live bindings");
    let handle = runtime.handle(&walker).expect("walker handle");
    let task = runtime.spawn();

    handle.send(WalkerMessage::Start).await.expect("start");
    assert_eq!(reports.recv().await.as_deref(), Some("first\nsecond\n"));
    assert_eq!(reports.recv().await.as_deref(), Some("replaced"));
    assert_eq!(reports.recv().await.as_deref(), Some("Read NotFound"));
    assert_eq!(reports.recv().await.as_deref(), Some("done"));

    assert!(
        root.join("notes").is_dir(),
        "parent directories are created"
    );
    assert!(!root.join(FILE).exists(), "the file was removed");
    let leftovers: Vec<_> = std::fs::read_dir(root.join("notes"))
        .expect("notes dir")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert!(leftovers.is_empty(), "no temp files remain: {leftovers:?}");

    assert!(task.shutdown(Shutdown::Drain).await.unwrap().is_clean());
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn bound_driver_rejects_paths_outside_the_root_without_touching_disk() {
    let root = unique_root("escape");
    let (program, walker) = walker_program();
    let (sender, mut reports) = tokio::sync::mpsc::unbounded_channel();
    let runtime = Runtime::builder(program)
        .bind_fs(&root)
        .bind_effect::<Report, _>(Reporter(sender))
        .build()
        .expect("valid live bindings");
    let handle = runtime.handle(&walker).expect("walker handle");
    let task = runtime.spawn();

    handle.send(WalkerMessage::Escape).await.expect("escape");
    assert_eq!(reports.recv().await.as_deref(), Some("Append OutsideRoot"));
    assert!(!root.exists(), "nothing was created");
    assert!(!root.parent().unwrap().join("escape.log").exists());

    assert!(task.shutdown(Shutdown::Drain).await.unwrap().is_clean());
}

#[test]
fn bind_fs_binds_only_declared_effects_and_participates_in_duplicate_validation() {
    let mut builder = Program::builder();
    let _append = builder.effect::<AppendToFile>();
    let program = builder.build().expect("append-only program");
    assert!(
        Runtime::builder(program).bind_fs("ignored").build().is_ok(),
        "a program that only appends needs no Read, Replace, or Remove binding"
    );

    let (program, _) = walker_program();
    assert!(
        Runtime::builder(program)
            .bind_fs("ignored")
            .bind_effect::<ReadFile, _>(FsDriver::new("elsewhere"))
            .build()
            .is_err(),
        "a second ReadFile binding is a duplicate"
    );
}
