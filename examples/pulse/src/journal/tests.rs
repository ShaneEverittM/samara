//! Journal acceptance: reads and appends use one file capability and do not retry.
//! Recent returns the tail of the supplied read result, including an unterminated
//! last line. Reads need not wait for appends or complete in request order; each
//! completion retains its own count and reply token.
//! Replies distinguish a successful tail from Journal's read-error and
//! cancellation variants; append completion never produces another transition.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::*;

type JournalNeeds = <Journal as Assembly>::Requires;
type JournalReply = Result<Vec<String>>;

fn transition() -> JournalMessage {
    JournalMessage::Transition {
        name: "pump".into(),
        from: "idle".into(),
        to: "running".into(),
    }
}

struct JournalClient;

enum ClientMessage {
    Transition,
    Recent(usize),
    Replied(usize, RequestOutcome<JournalReply>),
}

impl Component for JournalClient {
    type Requires = ();
    type Config = Connector<JournalMessage>;
    type Model = Vec<(usize, RequestOutcome<JournalReply>)>;
    type Message = ClientMessage;

    fn init(_: &Self::Config, _: &()) -> Init<Self::Model, ClientMessage> {
        Init::default()
    }

    fn update(
        journal: &Self::Config,
        _: &(),
        model: &mut Self::Model,
        message: ClientMessage,
    ) -> Command<ClientMessage> {
        match message {
            ClientMessage::Transition => Command::send(journal, transition()),
            ClientMessage::Recent(count) => Command::request_to(
                journal,
                move |reply| JournalMessage::Recent { count, reply },
                move |outcome| ClientMessage::Replied(count, outcome),
            ),
            ClientMessage::Replied(count, outcome) => {
                model.push((count, outcome));
                Command::none()
            }
        }
    }
}

fn program() -> (
    Program<JournalNeeds>,
    ComponentRef<JournalClient>,
    Port<JournalMessage>,
    FileCapability<ReadAppend>,
) {
    let (mut builder, capabilities) = Program::builder::<JournalNeeds>();
    let file = capabilities.0.clone();
    let journal = builder
        .install::<Journal>(())
        .with_capabilities(capabilities)
        .finish()
        .unwrap();
    let port = builder.port::<JournalMessage>(PortId::new("journal"));
    builder.connect(&port, journal.connector());
    let client = builder
        .component::<JournalClient>(journal.connector().clone())
        .finish()
        .unwrap();
    (builder.build().unwrap(), client, port, file)
}

fn successful_replies(
    runtime: &ControlledRuntime,
    client: &ComponentRef<JournalClient>,
) -> Vec<(usize, Vec<String>)> {
    runtime
        .state(client)
        .unwrap()
        .iter()
        .map(|(count, outcome)| match outcome {
            RequestOutcome::Replied(Ok(lines)) => (*count, lines.clone()),
            other => panic!("expected a successful Journal reply, got {other:?}"),
        })
        .collect()
}

#[test]
fn assembly_appends_through_its_supplied_file_capability() {
    let (program, client, _, file) = program();
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
        .build()
        .unwrap();
    runtime.send(&client, ClientMessage::Transition).unwrap();
    runtime.run_until_idle().unwrap();
    let pending = runtime.next_file_append(&file).unwrap();
    assert_eq!(pending.intent.as_str(), Some("pump: idle -> running\n"));
    runtime
        .complete(pending, EffectOutcome::Succeeded(()))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(runtime.pending_work().pending_later, 0);
    assert!(runtime.cancel().unwrap().is_clean());
}

#[test]
fn append_outcomes_are_discarded_without_automatic_retry() {
    let (mut builder, capabilities) = Program::builder::<JournalNeeds>();
    let file = capabilities.0.clone();
    let recorder = builder
        .component::<Recorder>(())
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
        EffectOutcome::Failed(EffectFailure::Error(failure)),
        EffectOutcome::Failed(EffectFailure::Cancelled(CancelReason::Superseded)),
    ] {
        runtime
            .send(&recorder, Message::Journal(transition()))
            .unwrap();
        assert_eq!(runtime.run_until_idle().unwrap().transitions, 1);
        let pending = runtime.next_file_append(&file).unwrap();
        runtime.complete(pending, outcome).unwrap();
        assert_eq!(runtime.run_until_idle().unwrap().transitions, 0);
        assert_eq!(runtime.pending_work(), PendingWork::default());
    }
    assert!(runtime.cancel().unwrap().is_clean());
}

#[test]
fn recent_returns_the_requested_tail_of_the_supplied_file() {
    let cases: &[(&str, usize, &[&str])] = &[
        ("", 3, &[]),
        ("first\nsecond\n", 0, &[]),
        ("first\nsecond\nthird\n", 2, &["second", "third"]),
        ("first\nsecond\n", 2, &["first", "second"]),
        ("first\nsecond\n", usize::MAX, &["first", "second"]),
        ("first\nunfinished", 1, &["unfinished"]),
        ("first\n\nthird\n", 2, &["", "third"]),
        ("first\r\nsecond\r\n", 1, &["second"]),
        ("first\ncafé\n東京\n", 2, &["café", "東京"]),
    ];
    for &(text, count, expected) in cases {
        let (program, client, _, file) = program();
        let mut runtime = ControlledRuntime::builder(program)
            .control_all()
            .build()
            .unwrap();
        runtime.send(&client, ClientMessage::Recent(count)).unwrap();
        runtime.run_until_idle().unwrap();
        let pending = runtime.next_file_read(&file).unwrap();
        assert!(runtime.state(&client).unwrap().is_empty());
        runtime
            .complete(
                pending,
                EffectOutcome::Succeeded(Bytes::copy_from_slice(text.as_bytes())),
            )
            .unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(
            successful_replies(&runtime, &client),
            vec![(count, expected.iter().map(|line| (*line).into()).collect())],
            "text={text:?}, count={count}",
        );
        assert_eq!(runtime.pending_work().pending_later, 0);
        assert!(runtime.cancel().unwrap().is_clean());
    }
}

#[test]
fn overlapping_recent_requests_keep_their_counts_and_reply_tokens() {
    let (program, client, _, file) = program();
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
        .build()
        .unwrap();
    runtime.send(&client, ClientMessage::Recent(1)).unwrap();
    runtime.run_until_idle().unwrap();
    let first = runtime.next_file_read(&file).unwrap();
    runtime.send(&client, ClientMessage::Recent(2)).unwrap();
    runtime.run_until_idle().unwrap();
    let second = runtime.next_file_read(&file).unwrap();

    runtime
        .complete(
            second,
            EffectOutcome::Succeeded(Bytes::from_static(b"alpha\nbeta\ngamma\n")),
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    let mut expected = vec![(2, vec!["beta".into(), "gamma".into()])];
    assert_eq!(successful_replies(&runtime, &client), expected);

    runtime
        .complete(
            first,
            EffectOutcome::Succeeded(Bytes::from_static(b"older\ncontents\n")),
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    expected.push((1, vec!["contents".into()]));
    assert_eq!(successful_replies(&runtime, &client), expected);
    assert_eq!(runtime.pending_work().pending_later, 0);
    assert!(runtime.cancel().unwrap().is_clean());
}

#[test]
fn recent_uses_its_read_result_regardless_of_append_completion_order() {
    for append_finishes_first in [false, true] {
        let (program, client, _, file) = program();
        let mut runtime = ControlledRuntime::builder(program)
            .control_all()
            .build()
            .unwrap();
        runtime.send(&client, ClientMessage::Transition).unwrap();
        runtime.run_until_idle().unwrap();
        let mut append = Some(runtime.next_file_append(&file).unwrap());
        runtime.send(&client, ClientMessage::Recent(1)).unwrap();
        runtime.run_until_idle().unwrap();
        let read = runtime.next_file_read(&file).unwrap();

        if append_finishes_first {
            runtime
                .complete(append.take().unwrap(), EffectOutcome::Succeeded(()))
                .unwrap();
            runtime.run_until_idle().unwrap();
        }
        runtime
            .complete(
                read,
                EffectOutcome::Succeeded(Bytes::from_static(b"previous entry\n")),
            )
            .unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(
            successful_replies(&runtime, &client),
            vec![(1, vec!["previous entry".into()])],
        );
        if let Some(append) = append {
            runtime
                .complete(append, EffectOutcome::Succeeded(()))
                .unwrap();
            runtime.run_until_idle().unwrap();
        }
        assert_eq!(runtime.pending_work().pending_later, 0);
        assert!(runtime.cancel().unwrap().is_clean());
    }
}

#[test]
fn read_failures_reply_with_the_journal_domain_error() {
    let failure = FsError::new(
        FsOperation::Read,
        "chosen.log",
        FsErrorKind::PermissionDenied,
        "read denied by fixture",
    );
    for (failure, expected_cancelled) in [
        (EffectFailure::Error(failure), false),
        (EffectFailure::Cancelled(CancelReason::Superseded), true),
        (EffectFailure::Cancelled(CancelReason::Deadline), true),
        (EffectFailure::Cancelled(CancelReason::Shutdown), true),
    ] {
        let (program, client, _, file) = program();
        let mut runtime = ControlledRuntime::builder(program)
            .control_all()
            .build()
            .unwrap();
        runtime.send(&client, ClientMessage::Recent(3)).unwrap();
        runtime.run_until_idle().unwrap();
        let read = runtime.next_file_read(&file).unwrap();
        runtime
            .complete(read, EffectOutcome::Failed(failure))
            .unwrap();
        runtime.run_until_idle().unwrap();

        let replies = runtime.state(&client).unwrap();
        let [(count, RequestOutcome::Replied(Err(error)))] = replies.as_slice() else {
            panic!("expected one Journal error reply, got {replies:?}");
        };
        assert_eq!(*count, 3);
        assert!(matches!(
            (expected_cancelled, error),
            (false, Error::CouldNotReadJournal) | (true, Error::JournalReadCanceled)
        ));
        assert_eq!(runtime.pending_work(), PendingWork::default());
        assert!(runtime.cancel().unwrap().is_clean());
    }
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
async fn file_binding_selects_the_journal_target() {
    let root = TestRoot::new();
    let file = "custom/events.log";
    let (program, _, port, _) = program();
    let runtime = Runtime::builder(program)
        .with_drivers((FileDriver::new(root.0.join(file)), SystemClockDriver))
        .build()
        .unwrap();
    let journal = runtime.port_handle(&port).unwrap();
    assert!(
        !root.0.exists(),
        "configuration and binding must not create files"
    );
    let task = runtime.spawn();
    journal.send(transition()).await.unwrap();
    assert!(task.shutdown(Shutdown::Drain).await.unwrap().is_clean());
    assert_eq!(
        std::fs::read_to_string(root.0.join(file)).unwrap(),
        "pump: idle -> running\n"
    );
}

#[tokio::test]
async fn recent_reads_existing_entries_through_the_standard_driver() {
    let root = TestRoot::new();
    let file = "custom/events.log";
    std::fs::create_dir_all(root.0.join("custom")).unwrap();
    std::fs::write(root.0.join(file), "first\nsecond\nthird\n").unwrap();
    let (program, _, port, _) = program();
    let runtime = Runtime::builder(program)
        .with_drivers((FileDriver::new(root.0.join(file)), SystemClockDriver))
        .build()
        .unwrap();
    let journal = runtime.port_handle(&port).unwrap();
    let task = runtime.spawn();
    let outcome = journal
        .request_to_timeout(
            |reply| JournalMessage::Recent { count: 2, reply },
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    let RequestOutcome::Replied(Ok(lines)) = outcome else {
        panic!("expected a successful Journal reply, got {outcome:?}");
    };
    assert_eq!(lines, ["second", "third"]);

    std::fs::remove_file(root.0.join(file)).unwrap();
    let missing = journal
        .request_to_timeout(
            |reply| JournalMessage::Recent { count: 2, reply },
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert!(matches!(
        missing,
        RequestOutcome::Replied(Err(Error::CouldNotReadJournal))
    ));
    assert!(task.shutdown(Shutdown::Drain).await.unwrap().is_clean());
}
