//! Journal acceptance: reads and appends use one file capability and do not retry.
//! A transition reads the clock first and is appended with that time as a prefix.
//! Recent returns the tail of the supplied read result, including an unterminated
//! last line. Reads need not wait for appends or complete in request order; each
//! completion retains its own count and reply token.
//! Replies distinguish a successful tail from Journal's read-error and
//! cancellation variants; append completion never produces another transition.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::*;

type JournalReply = Result<Vec<String>>;

/// The time every controlled test supplies, and the line it produces.
const STAMP_MILLIS: u64 = 1_700_000_000_000;
const STAMPED_LINE: &str = "[1700000000000]pump: idle -> running\n";

/// Answers the Recorder's pending clock read so the append can be issued.
fn supply_time(runtime: &mut ControlledRuntime) {
    let clock = runtime.next_effect::<ReadSystemTime>().unwrap();
    runtime
        .complete(
            clock,
            EffectOutcome::Succeeded(UNIX_EPOCH + Duration::from_millis(STAMP_MILLIS)),
        )
        .unwrap();
}

fn transition() -> JournalMessage {
    JournalMessage::Transition(Transition {
        name: "pump".into(),
        from: "idle".into(),
        to: "running".into(),
    })
}

struct JournalClient;

enum ClientMessage {
    Transition,
    Recent(usize),
    Replied(usize, RequestOutcome<JournalReply>),
}

impl Component for JournalClient {
    type Io = ();
    type Connections = Connector<JournalMessage>;
    type Configuration = ();
    type Model = Vec<(usize, RequestOutcome<JournalReply>)>;
    type Message = ClientMessage;

    fn init(_: &Context<Self>) -> Init<Self::Model, ClientMessage> {
        Init::default()
    }

    fn update(
        cx: &Context<Self>,
        model: &mut Self::Model,
        message: ClientMessage,
    ) -> Command<ClientMessage> {
        match message {
            ClientMessage::Transition => Command::send(&cx.connections, transition()),
            ClientMessage::Recent(count) => Command::request(&cx.connections, move |reply| {
                JournalMessage::Recent { count, reply }
            })
            .producing(move |outcome| ClientMessage::Replied(count, outcome)),
            ClientMessage::Replied(count, outcome) => {
                model.push((count, outcome));
                Command::none()
            }
        }
    }
}

/// The Program, a client that talks to the journal, a Port for live hosts,
/// and the journal's own capabilities for selecting effects and supplying
/// Drivers.
fn program() -> (
    Program,
    ComponentRef<JournalClient>,
    Port<JournalMessage>,
    JournalIo,
) {
    let mut builder = Program::builder();
    let journal = builder.add_assembly::<Journal>(());
    let port = builder.add_port::<JournalMessage>(PortId::new("journal"));
    builder.connect(&port, &journal.connector());
    let client = builder.add_component::<JournalClient>(journal.connector());
    let io = journal.io.clone();
    (builder.build().unwrap(), client.into(), port, io)
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
    let (program, client, _, JournalIo { file, .. }) = program();
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
        .build()
        .unwrap();
    runtime.send(&client, ClientMessage::Transition).unwrap();
    runtime.run_until_idle().unwrap();
    supply_time(&mut runtime);
    runtime.run_until_idle().unwrap();
    let pending = runtime.next_file_append(&file).unwrap();
    assert_eq!(pending.intent.as_str(), Some(STAMPED_LINE));
    runtime
        .complete(pending, EffectOutcome::Succeeded(()))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(runtime.pending_work().pending_later, 0);
    assert!(runtime.cancel().unwrap().is_clean());
}

#[test]
fn append_outcomes_are_discarded_without_automatic_retry() {
    let mut builder = Program::builder();
    let recorder = builder.add_component::<Recorder>(());
    let file = recorder.io.file.clone();
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
    for outcome in [EffectOutcome::Succeeded(()), EffectOutcome::Failed(failure)] {
        runtime
            .send(&recorder, Message::Journal(transition()))
            .unwrap();
        assert_eq!(runtime.run_until_idle().unwrap().transitions, 1);
        supply_time(&mut runtime);
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
        let (program, client, _, JournalIo { file, .. }) = program();
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
    let (program, client, _, JournalIo { file, .. }) = program();
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
        let (program, client, _, JournalIo { file, .. }) = program();
        let mut runtime = ControlledRuntime::builder(program)
            .control_all()
            .build()
            .unwrap();
        runtime.send(&client, ClientMessage::Transition).unwrap();
        runtime.run_until_idle().unwrap();
        supply_time(&mut runtime);
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
    let (program, client, _, JournalIo { file, .. }) = program();
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
    assert!(matches!(error, Error::CouldNotReadJournal));
    assert_eq!(runtime.pending_work(), PendingWork::default());
    assert!(runtime.cancel().unwrap().is_clean());
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
    let (program, _, port, io) = program();
    let runtime = Runtime::builder(program)
        .supply(&io.file, FileDriver::new(root.0.join(file)))
        .supply(&io.clock, SystemClockDriver)
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
    // The live clock supplies a real time, so check the shape of the line.
    let written = std::fs::read_to_string(root.0.join(file)).unwrap();
    let (stamp, entry) = written
        .strip_prefix('[')
        .and_then(|rest| rest.split_once(']'))
        .expect("the line starts with a bracketed timestamp");
    assert!(stamp.parse::<u128>().is_ok(), "{stamp}");
    assert_eq!(entry, "pump: idle -> running\n");
}

#[tokio::test]
async fn recent_reads_existing_entries_through_the_standard_driver() {
    let root = TestRoot::new();
    let file = "custom/events.log";
    std::fs::create_dir_all(root.0.join("custom")).unwrap();
    std::fs::write(root.0.join(file), "first\nsecond\nthird\n").unwrap();
    let (program, _, port, io) = program();
    let runtime = Runtime::builder(program)
        .supply(&io.file, FileDriver::new(root.0.join(file)))
        .supply(&io.clock, SystemClockDriver)
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

/// The journal's whole access to the world and who sends to it, pinned so a
/// new capability or connection shows up as a failing test and a reviewed
/// diff.
#[test]
fn the_journal_reaches_one_file_and_the_clock() {
    let (program, ..) = program();
    assert_eq!(
        program.manifest().to_string(),
        "pulse::journal::Recorder.file: effect samara::file::FileRead\n\
         pulse::journal::Recorder.file: effect samara::file::FileAppend\n\
         pulse::journal::Recorder.clock: effect samara::clock::ReadSystemTime\n\
         pulse::journal::tests::JournalClient: connection pulse::journal::JournalMessage \
         -> pulse::journal::Recorder\n"
    );
}
