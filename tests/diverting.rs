//! The effect finishers (ADR-0023): each is named for what its function
//! receives, and `divert_to` sends the result to a reply token or an address
//! with no Message in between.

use samara::prelude::*;

/// A lookup that can fail.
#[derive(Debug, PartialEq)]
struct Lookup(&'static str);

#[derive(Debug, PartialEq)]
struct Missing;

impl EffectDescriptor for Lookup {
    type Output = u64;
    type Error = Missing;
}

/// A reading that cannot fail.
#[derive(Debug, PartialEq)]
struct Reading;

impl EffectDescriptor for Reading {
    type Output = u64;
    type Error = !;
}

#[derive(Debug)]
enum DeskMessage {
    /// Reply through one arm per case.
    Find(&'static str, ReplyWith<Result<u64, String>>),
    /// Reply with the output of an effect that cannot fail.
    Read(ReplyWith<u64>),
    /// Reply with the whole outcome, described.
    Describe(&'static str, ReplyWith<String>),
    /// Tell the log instead of the caller.
    Log(&'static str),
    /// Look up and keep the result here.
    Keep(&'static str),
    Kept(u64),
    Lost,
}

struct Desk;

#[derive(Clone, Io)]
struct DeskIo {
    lookup: Effect<Lookup>,
    reading: Effect<Reading>,
}

#[derive(Debug, Default, PartialEq)]
struct DeskModel {
    /// How many Messages `update` has handled.
    handled: u64,
    kept: Vec<Option<u64>>,
}

impl Component for Desk {
    type Io = DeskIo;
    type Connections = ComponentRef<Log>;
    type Configuration = ();
    type Model = DeskModel;
    type Message = DeskMessage;

    fn init(_: &Context<Self>) -> Init<DeskModel, DeskMessage> {
        Init::default()
    }

    fn update(
        cx: &Context<Self>,
        model: &mut DeskModel,
        message: DeskMessage,
    ) -> Command<DeskMessage> {
        model.handled += 1;
        match message {
            DeskMessage::Find(key, reply) => {
                Effect::perform(&cx.io.lookup, Lookup(key))
                    .divert_to(reply)
                    .on_success(|value| Ok(value * 2))
                    .on_failure(move |Missing| Err(format!("no {key}")))
            }
            DeskMessage::Read(reply) => {
                Effect::perform(&cx.io.reading, Reading)
                    .divert_to(reply)
                    .on_success(|value| value + 1)
            }
            DeskMessage::Describe(key, reply) => {
                Effect::perform(&cx.io.lookup, Lookup(key))
                    .divert_to(reply)
                    .on_outcome(|outcome| format!("{outcome:?}"))
            }
            DeskMessage::Log(key) => {
                Effect::perform(&cx.io.lookup, Lookup(key))
                    .divert_to(&cx.to)
                    .on_success(move |value| format!("{key} = {value}"))
                    .on_failure(move |Missing| format!("{key} missing"))
            }
            DeskMessage::Keep(key) => {
                Effect::perform(&cx.io.lookup, Lookup(key))
                    .on_success(DeskMessage::Kept)
                    .on_failure(|Missing| DeskMessage::Lost)
            }
            DeskMessage::Kept(value) => {
                model.kept.push(Some(value));
                Command::none()
            }
            DeskMessage::Lost => {
                model.kept.push(None);
                Command::none()
            }
        }
    }
}

/// Collects what it is told.
struct Log;

impl Component for Log {
    type Io = ();
    type Connections = ();
    type Configuration = ();
    type Model = Vec<String>;
    type Message = String;

    fn init(_: &Context<Self>) -> Init<Vec<String>, String> {
        Init::default()
    }

    fn update(_: &Context<Self>, lines: &mut Vec<String>, line: String) -> Command<String> {
        lines.push(line);
        Command::none()
    }
}

#[derive(Debug, PartialEq)]
enum Answer {
    Found(RequestOutcome<Result<u64, String>>),
    Read(RequestOutcome<u64>),
    Described(RequestOutcome<String>),
}

enum ClientMessage {
    Find(&'static str),
    Read,
    Describe(&'static str),
    Answered(Answer),
}

struct Client;

impl Component for Client {
    type Io = ();
    type Connections = Connector<DeskMessage>;
    type Configuration = ();
    type Model = Vec<Answer>;
    type Message = ClientMessage;

    fn init(_: &Context<Self>) -> Init<Vec<Answer>, ClientMessage> {
        Init::default()
    }

    fn update(
        cx: &Context<Self>,
        answers: &mut Vec<Answer>,
        message: ClientMessage,
    ) -> Command<ClientMessage> {
        match message {
            ClientMessage::Find(key) => {
                Command::request(&cx.to, move |reply| DeskMessage::Find(key, reply))
                    .on_outcome(|outcome| ClientMessage::Answered(Answer::Found(outcome)))
            }
            ClientMessage::Read => {
                Command::request(&cx.to, DeskMessage::Read)
                    .on_outcome(|outcome| ClientMessage::Answered(Answer::Read(outcome)))
            }
            ClientMessage::Describe(key) => {
                Command::request(&cx.to, move |reply| DeskMessage::Describe(key, reply))
                    .on_outcome(|outcome| ClientMessage::Answered(Answer::Described(outcome)))
            }
            ClientMessage::Answered(answer) => {
                answers.push(answer);
                Command::none()
            }
        }
    }
}

struct Fixture {
    runtime: ControlledRuntime,
    desk: InstalledComponent<Desk>,
    log: ComponentRef<Log>,
    client: ComponentRef<Client>,
}

fn fixture() -> Fixture {
    let mut builder = Program::builder();
    let log: ComponentRef<Log> = builder.add_component::<Log>(()).into();
    let desk = builder.add_component::<Desk>(log.clone());
    let client = builder.add_component::<Client>(desk.connector());
    let runtime = ControlledRuntime::builder(builder.build().unwrap())
        .build()
        .unwrap();
    Fixture {
        runtime,
        desk,
        log,
        client: client.into(),
    }
}

/// Runs to idle, completes the one pending lookup, and runs to idle again.
fn complete_lookup(runtime: &mut ControlledRuntime, outcome: EffectOutcome<u64, Missing>) {
    runtime.run_until_idle().unwrap();
    let lookup = runtime.next_effect::<Lookup>().unwrap();
    runtime.complete(lookup, outcome).unwrap();
    runtime.run_until_idle().unwrap();
}

#[test]
fn a_diverted_reply_goes_through_one_arm_per_case() {
    let Fixture {
        mut runtime,
        desk,
        client,
        ..
    } = fixture();

    runtime.send(&client, ClientMessage::Find("a")).unwrap();
    complete_lookup(&mut runtime, EffectOutcome::Succeeded(21));
    runtime.send(&client, ClientMessage::Find("b")).unwrap();
    complete_lookup(&mut runtime, EffectOutcome::Failed(Missing));

    assert_eq!(
        runtime.state(&client).unwrap(),
        &[
            Answer::Found(RequestOutcome::Replied(Ok(42))),
            Answer::Found(RequestOutcome::Replied(Err("no b".into()))),
        ]
    );
    // The outcome never came back: the Desk handled only the two requests.
    assert_eq!(runtime.state(&desk).unwrap().handled, 2);
    assert_eq!(runtime.pending_work(), PendingWork::default());
    assert!(runtime.cancel().unwrap().is_clean());
}

#[test]
fn a_diverted_effect_that_cannot_fail_needs_one_function() {
    let Fixture {
        mut runtime,
        desk,
        client,
        ..
    } = fixture();

    runtime.send(&client, ClientMessage::Read).unwrap();
    runtime.run_until_idle().unwrap();
    let reading = runtime.next_effect::<Reading>().unwrap();
    runtime
        .complete(reading, EffectOutcome::Succeeded(6))
        .unwrap();
    runtime.run_until_idle().unwrap();

    assert_eq!(
        runtime.state(&client).unwrap(),
        &[Answer::Read(RequestOutcome::Replied(7))]
    );
    assert_eq!(runtime.state(&desk).unwrap().handled, 1);
    assert!(runtime.cancel().unwrap().is_clean());
}

#[test]
fn a_diverted_reply_can_map_the_whole_outcome() {
    let Fixture {
        mut runtime,
        client,
        ..
    } = fixture();

    runtime.send(&client, ClientMessage::Describe("a")).unwrap();
    complete_lookup(&mut runtime, EffectOutcome::Failed(Missing));

    assert_eq!(
        runtime.state(&client).unwrap(),
        &[Answer::Described(RequestOutcome::Replied(
            "Failed(Missing)".into()
        ))]
    );
    assert!(runtime.cancel().unwrap().is_clean());
}

#[test]
fn a_result_can_be_diverted_to_an_address() {
    let Fixture {
        mut runtime,
        desk,
        log,
        ..
    } = fixture();

    runtime.send(&desk, DeskMessage::Log("a")).unwrap();
    complete_lookup(&mut runtime, EffectOutcome::Succeeded(3));
    runtime.send(&desk, DeskMessage::Log("b")).unwrap();
    complete_lookup(&mut runtime, EffectOutcome::Failed(Missing));

    assert_eq!(runtime.state(&log).unwrap(), &["a = 3", "b missing"]);
    assert_eq!(runtime.state(&desk).unwrap().handled, 2);
    assert!(runtime.cancel().unwrap().is_clean());
}

#[test]
fn without_divert_the_arms_come_back_as_messages() {
    let Fixture {
        mut runtime, desk, ..
    } = fixture();

    runtime.send(&desk, DeskMessage::Keep("a")).unwrap();
    complete_lookup(&mut runtime, EffectOutcome::Succeeded(5));
    runtime.send(&desk, DeskMessage::Keep("b")).unwrap();
    complete_lookup(&mut runtime, EffectOutcome::Failed(Missing));

    assert_eq!(
        runtime.state(&desk).unwrap(),
        &DeskModel {
            // Two requests, and each result handled as a Message.
            handled: 4,
            kept: vec![Some(5), None],
        }
    );
    assert!(runtime.cancel().unwrap().is_clean());
}
