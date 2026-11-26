//! `replying` answers a request with an effect's result: one arm for
//! success, one for failure, and no Message in between (ADR-0023).

use std::convert::Infallible;

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
    type Error = Infallible;
}

#[derive(Debug)]
enum DeskMessage {
    Find(&'static str, ReplyWith<Result<u64, String>>),
    Read(ReplyWith<u64>),
}

struct Desk;

#[derive(Clone, Io)]
struct DeskIo {
    lookup: Effect<Lookup>,
    reading: Effect<Reading>,
}

impl Component for Desk {
    type Io = DeskIo;
    type Connections = ();
    type Configuration = ();
    type Model = u64;
    type Message = DeskMessage;

    fn init(_: &Context<Self>) -> Init<u64, DeskMessage> {
        Init::new(0)
    }

    fn update(cx: &Context<Self>, handled: &mut u64, message: DeskMessage) -> Command<DeskMessage> {
        *handled += 1;
        match message {
            DeskMessage::Find(key, reply) => {
                Effect::perform(&cx.io.lookup, Lookup(key))
                    .replying(reply)
                    .succeeding(|value| Ok(value * 2))
                    .failing(move |Missing| Err(format!("no {key}")))
            }
            DeskMessage::Read(reply) => {
                Effect::perform(&cx.io.reading, Reading)
                    .replying(reply)
                    .succeeding(|value| value + 1)
                    .into_command()
            }
        }
    }
}

#[derive(Debug, PartialEq)]
enum Answer {
    Found(RequestOutcome<Result<u64, String>>),
    Read(RequestOutcome<u64>),
}

enum ClientMessage {
    Find(&'static str),
    Read,
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
                    .producing(|outcome| ClientMessage::Answered(Answer::Found(outcome)))
            }
            ClientMessage::Read => {
                Command::request(&cx.to, DeskMessage::Read)
                    .producing(|outcome| ClientMessage::Answered(Answer::Read(outcome)))
            }
            ClientMessage::Answered(answer) => {
                answers.push(answer);
                Command::none()
            }
        }
    }
}

fn runtime() -> (
    ControlledRuntime,
    InstalledComponent<Desk>,
    ComponentRef<Client>,
) {
    let mut builder = Program::builder();
    let desk = builder.add_component::<Desk>(());
    let client = builder.add_component::<Client>(desk.connector());
    let runtime = ControlledRuntime::builder(builder.build().unwrap())
        .build()
        .unwrap();
    (runtime, desk, client.into())
}

#[test]
fn success_and_failure_each_reply_through_their_own_arm() {
    let (mut runtime, desk, client) = runtime();

    runtime.send(&client, ClientMessage::Find("a")).unwrap();
    runtime.run_until_idle().unwrap();
    let lookup = runtime.next_effect::<Lookup>().unwrap();
    runtime
        .complete(lookup, EffectOutcome::Succeeded(21))
        .unwrap();
    runtime.run_until_idle().unwrap();

    runtime.send(&client, ClientMessage::Find("b")).unwrap();
    runtime.run_until_idle().unwrap();
    let lookup = runtime.next_effect::<Lookup>().unwrap();
    runtime
        .complete(lookup, EffectOutcome::Failed(Missing))
        .unwrap();
    runtime.run_until_idle().unwrap();

    assert_eq!(
        runtime.state(&client).unwrap(),
        &[
            Answer::Found(RequestOutcome::Replied(Ok(42))),
            Answer::Found(RequestOutcome::Replied(Err("no b".into()))),
        ]
    );
    // The outcome replied directly: the Desk handled only the two requests.
    assert_eq!(runtime.state(&desk).unwrap(), &2);
    assert_eq!(runtime.pending_work(), PendingWork::default());
    assert!(runtime.cancel().unwrap().is_clean());
}

#[test]
fn an_infallible_effect_needs_only_the_success_arm() {
    let (mut runtime, desk, client) = runtime();

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
    assert_eq!(runtime.state(&desk).unwrap(), &1);
    assert!(runtime.cancel().unwrap().is_clean());
}
