//! Acceptance evidence for the builder form of boundary work (ADR-0023):
//! the verb lives on the thing a Component holds, every interaction ends
//! with one of four finishers, and `finish_with` runs exactly one hop of
//! work that can produce no Message.

use std::convert::Infallible;
use std::time::Duration;

use samara::prelude::*;

/// A measurement the provider must make before it can answer.
#[derive(Debug, PartialEq)]
struct Probe;

impl EffectDescriptor for Probe {
    type Output = u64;
    type Error = Infallible;
}

/// The provider's vocabulary: ask for a reading, or ask and never hear back.
#[derive(Debug)]
enum GaugeMessage {
    Read(ReplyWith<u64>),
    Park(ReplyWith<u64>),
    Note(String),
}

struct Gauge;

#[derive(Default)]
struct GaugeModel {
    parked: Vec<ReplyWith<u64>>,
    notes: Vec<String>,
}

impl Component for Gauge {
    type Io = Effect<Probe>;
    type Config = ();
    type Model = GaugeModel;
    type Message = GaugeMessage;

    fn init(_: &(), _: &Self::Io) -> Init<GaugeModel, GaugeMessage> {
        Init::default()
    }

    fn update(
        _: &(),
        probe: &Self::Io,
        model: &mut GaugeModel,
        message: GaugeMessage,
    ) -> Command<GaugeMessage> {
        match message {
            // Answer the caller straight from the effect's outcome: no private
            // Message variant carries the reply token across the hop.
            GaugeMessage::Read(reply) => {
                Effect::perform(probe, Probe).finish_with(move |outcome| {
                    Command::reply(
                        reply,
                        match outcome {
                            EffectOutcome::Succeeded(reading) => reading,
                            EffectOutcome::Failed(_) => 0,
                        },
                    )
                })
            }
            GaugeMessage::Park(reply) => {
                model.parked.push(reply);
                Command::none()
            }
            GaugeMessage::Note(note) => {
                model.notes.push(note);
                Command::none()
            }
        }
    }
}

/// Sits between a client and the gauge and adds one to whatever it reads.
struct Relay;

#[derive(Debug)]
enum RelayMessage {
    ReadPlusOne(ReplyWith<u64>),
}

impl Component for Relay {
    type Io = ();
    type Config = Connector<GaugeMessage>;
    type Model = ();
    type Message = RelayMessage;

    fn init(_: &Self::Config, _: &()) -> Init<(), RelayMessage> {
        Init::default()
    }

    fn update(
        gauge: &Self::Config,
        _: &(),
        _: &mut (),
        RelayMessage::ReadPlusOne(reply): RelayMessage,
    ) -> Command<RelayMessage> {
        let audit = gauge.clone();
        Command::request(&gauge, GaugeMessage::Read).finish_with(move |outcome| {
            let reading = match outcome {
                RequestOutcome::Replied(reading) => reading + 1,
                _ => 0,
            };
            // Finishing work may be several things, none of which come back.
            Command::batch([
                Command::reply(reply, reading),
                Command::send(&audit, GaugeMessage::Note(format!("relayed {reading}"))),
            ])
        })
    }
}

struct Client;

struct ClientConfig {
    relay: Connector<RelayMessage>,
    gauge: Connector<GaugeMessage>,
}

#[derive(Debug, PartialEq)]
enum ClientMessage {
    Start,
    StartParked(Duration),
    Got(RequestOutcome<u64>),
}

impl From<RequestOutcome<u64>> for ClientMessage {
    fn from(outcome: RequestOutcome<u64>) -> Self {
        Self::Got(outcome)
    }
}

impl Component for Client {
    type Io = ();
    type Config = ClientConfig;
    type Model = Vec<RequestOutcome<u64>>;
    type Message = ClientMessage;

    fn init(_: &ClientConfig, _: &()) -> Init<Self::Model, ClientMessage> {
        Init::default()
    }

    fn update(
        config: &ClientConfig,
        _: &(),
        model: &mut Self::Model,
        message: ClientMessage,
    ) -> Command<ClientMessage> {
        match message {
            ClientMessage::Start => Command::request(&config.relay, RelayMessage::ReadPlusOne)
                .producing(ClientMessage::Got),
            ClientMessage::StartParked(patience) => {
                Command::request(&config.gauge, GaugeMessage::Park)
                    .timeout(patience)
                    .into_command()
            }
            ClientMessage::Got(outcome) => {
                model.push(outcome);
                Command::none()
            }
        }
    }
}

struct Fixture {
    program: Program,
    client: ComponentRef<Client>,
    gauge: InstalledComponent<Gauge>,
    relay_port: Port<RelayMessage>,
}

fn fixture() -> Fixture {
    let mut builder = Program::builder();
    let gauge = builder.component::<Gauge>(());
    let relay = builder.component::<Relay>(gauge.connector());
    let relay_port = builder.port::<RelayMessage>(PortId::new("relay"));
    builder.connect(&relay_port, &relay.connector());
    let client = builder.component::<Client>(ClientConfig {
        relay: relay.connector(),
        gauge: gauge.connector(),
    });
    Fixture {
        program: builder.build().unwrap(),
        client: client.into(),
        gauge,
        relay_port,
    }
}

#[test]
fn finish_with_answers_through_an_effect_and_a_request_without_extra_transitions() {
    let Fixture {
        program,
        client,
        gauge,
        ..
    } = fixture();
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
        .build()
        .unwrap();

    runtime.send(&client, ClientMessage::Start).unwrap();
    // Client::Start, Relay::ReadPlusOne, Gauge::Read.
    assert_eq!(runtime.run_until_idle().unwrap().transitions, 3);
    let probe = runtime.next_effect::<Probe>().unwrap();
    assert!(runtime.state(&client).unwrap().is_empty());

    runtime
        .complete(probe, EffectOutcome::Succeeded(41))
        .unwrap();
    // The effect outcome replies to the relay, whose continuation replies to
    // the client and notes the gauge. Only the note and the client's reply are
    // transitions: neither finishing continuation re-entered its Component.
    assert_eq!(runtime.run_until_idle().unwrap().transitions, 2);
    assert_eq!(
        runtime.state(&client).unwrap(),
        &[RequestOutcome::Replied(42)]
    );
    assert_eq!(runtime.state(&gauge).unwrap().notes, ["relayed 42"]);
    assert_eq!(runtime.pending_work(), PendingWork::default());
    assert!(runtime.cancel().unwrap().is_clean());
}

#[test]
fn finishing_commands_are_traced_as_caused_by_the_outcome() {
    let Fixture {
        program, client, ..
    } = fixture();
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
        .build()
        .unwrap();
    runtime.send(&client, ClientMessage::Start).unwrap();
    runtime.run_until_idle().unwrap();
    let probe = runtime.next_effect::<Probe>().unwrap();
    runtime
        .complete(probe, EffectOutcome::Succeeded(1))
        .unwrap();
    runtime.run_until_idle().unwrap();

    let trace = runtime.trace();
    let reply = trace
        .iter()
        .find(|record| {
            matches!(
                record.event,
                TraceEvent::CommandEmitted {
                    kind: TraceCommandKind::Reply,
                    ..
                }
            )
        })
        .expect("the gauge's finishing reply is traced");
    let parent = &trace[reply.cause.expect("a finishing reply has a cause").get() as usize];
    assert!(
        matches!(parent.event, TraceEvent::EffectOutcome { .. }),
        "the reply's cause is the effect outcome, not a transition: {:?}",
        parent.event
    );
}

#[test]
fn a_request_builder_carries_its_timeout_and_from_conversion() {
    let Fixture {
        program, client, ..
    } = fixture();
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
        .build()
        .unwrap();
    let patience = Duration::from_secs(1);
    runtime
        .send(&client, ClientMessage::StartParked(patience))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.state(&client).unwrap().is_empty());
    runtime.advance(patience).unwrap();
    assert_eq!(runtime.state(&client).unwrap(), &[RequestOutcome::TimedOut]);
    assert!(runtime.cancel().unwrap().is_clean());
}

#[test]
fn inspectors_see_builder_commands_and_skip_finishing_requests() {
    let Fixture { program, .. } = fixture();
    drop(program);
    let mut builder = Program::builder();
    let gauge = builder.component::<Gauge>(());
    let probe = gauge.io.clone();
    let gauge = gauge.connector();

    let sent: Command<ClientMessage> = Command::send(&gauge, GaugeMessage::Note("hello".into()));
    assert!(matches!(
        sent.send_intents::<GaugeMessage>().as_slice(),
        [GaugeMessage::Note(note)] if note == "hello"
    ));

    let performed: Command<ClientMessage> = Effect::perform(&probe, Probe).discard_outcome();
    assert_eq!(performed.effect_intent::<Probe>(), Some(&Probe));

    let asked: Command<ClientMessage> = Command::request(&gauge, GaugeMessage::Read).into_command();
    let intent = asked
        .into_request::<GaugeMessage>()
        .ok()
        .expect("a Message-producing request is inspectable");
    assert!(matches!(intent.message, GaugeMessage::Read(_)));
    assert_eq!(
        intent.reply(RequestOutcome::Replied(9u64)),
        ClientMessage::Got(RequestOutcome::Replied(9))
    );

    // A finishing request has no Message to map to, so it is not extracted;
    // controlled execution observes it instead.
    let finishing: Command<ClientMessage> =
        Command::request(&gauge, GaugeMessage::Read).finish_with(|_| Command::none());
    assert!(finishing.into_request::<GaugeMessage>().is_err());
}

struct FixedProbe(u64);

impl EffectDriver<Probe> for FixedProbe {
    fn execute(&self, _: Probe) -> BoxFuture<Result<u64, Infallible>> {
        let reading = self.0;
        Box::pin(async move { Ok(reading) })
    }
}

#[tokio::test]
async fn finish_with_runs_the_same_way_live() -> Result<(), RuntimeError> {
    let Fixture {
        program,
        gauge,
        relay_port,
        ..
    } = fixture();
    let runtime = Runtime::builder(program)
        .supply(&gauge.io, FixedProbe(9))
        .build()?;
    let relay = runtime.port_handle(&relay_port)?;
    let task = runtime.spawn();
    assert_eq!(relay.request_to(RelayMessage::ReadPlusOne).await?, 10);
    assert!(task.shutdown(Shutdown::Drain).await?.is_clean());
    Ok(())
}
