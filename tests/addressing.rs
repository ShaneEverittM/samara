//! Acceptance evidence for `Addressable`: `Command::send` and
//! `Command::request` reach a Port exactly as they reach a Connector, a
//! hand-written message enum needs no macro or request trait, `PortHandle`
//! offers the same two forms to a live host, and `into_request` lets a
//! unit test inspect a request without a runtime.

use std::time::Duration;

use samara::prelude::*;

/// The consumer's vocabulary, written by hand. The request carries its own
/// reply token, which is what types the reply.
#[derive(Debug)]
enum LedgerMessage {
    Add(u64),
    Total(ReplyWith<u64>),
    Park(ReplyWith<u64>),
}

struct Ledger;

enum LedgerProviderMessage {
    Ledger(LedgerMessage),
}

#[derive(Default)]
struct LedgerModel {
    total: u64,
    parked: Vec<ReplyWith<u64>>,
}

impl Component for Ledger {
    type Io = ();
    type Connections = ();
    type Configuration = ();
    type Model = LedgerModel;
    type Message = LedgerProviderMessage;

    fn init(_: &Context<Self>) -> Init<LedgerModel, LedgerProviderMessage> {
        Init::new(LedgerModel::default())
    }

    fn update(
        _: &Context<Self>,
        model: &mut LedgerModel,
        LedgerProviderMessage::Ledger(message): LedgerProviderMessage,
    ) -> Command<LedgerProviderMessage> {
        match message {
            LedgerMessage::Add(amount) => {
                model.total += amount;
                Command::none()
            }
            LedgerMessage::Total(reply_with) => Command::reply(reply_with, model.total),
            LedgerMessage::Park(reply_with) => {
                model.parked.push(reply_with);
                Command::none()
            }
        }
    }
}

struct Clerk;

#[derive(Debug, Default, PartialEq, Eq)]
struct ClerkModel {
    total: Option<RequestOutcome<u64>>,
}

#[derive(Debug)]
enum ClerkMessage {
    Start,
    Total(RequestOutcome<u64>),
}

impl Component for Clerk {
    type Io = ();
    type Connections = Port<LedgerMessage>;
    /// `None` asks for the total; `Some` parks a request with that timeout.
    type Configuration = Option<Duration>;
    type Model = ClerkModel;
    type Message = ClerkMessage;

    fn init(_: &Context<Self>) -> Init<ClerkModel, ClerkMessage> {
        Init::new(ClerkModel::default())
    }

    fn update(
        cx: &Context<Self>,
        model: &mut ClerkModel,
        message: ClerkMessage,
    ) -> Command<ClerkMessage> {
        match message {
            ClerkMessage::Start => Command::batch([
                Command::send(&cx.to, LedgerMessage::Add(2)),
                Command::send(&cx.to, LedgerMessage::Add(3)),
                match cx.config {
                    None => Command::request(&cx.to, LedgerMessage::Total)
                        .producing(ClerkMessage::Total),
                    Some(patience) => Command::request(&cx.to, LedgerMessage::Park)
                        .timeout(patience)
                        .producing(ClerkMessage::Total),
                },
            ]),
            ClerkMessage::Total(outcome) => {
                model.total = Some(outcome);
                Command::none()
            }
        }
    }
}

fn assemble(patience: Option<Duration>) -> (Program, Port<LedgerMessage>, ComponentRef<Clerk>) {
    let mut program = Program::builder();
    let ledger = program.add_port::<LedgerMessage>(PortId::new("ledger"));
    let provider = program
        .configure_component::<Ledger>()
        .name("ledger")
        .add(());
    program.connect(&ledger, &provider.accepting(LedgerProviderMessage::Ledger));
    let clerk = program
        .configure_component::<Clerk>()
        .name("clerk")
        .config(patience)
        .add(ledger.clone());
    (
        program.build().expect("program assembles"),
        ledger,
        clerk.into(),
    )
}

#[test]
fn send_and_request_to_reach_a_port_through_its_binding() {
    let (program, _, clerk) = assemble(None);
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
        .build()
        .expect("nothing to control");

    runtime.send(&clerk, ClerkMessage::Start).expect("start");
    runtime
        .run_until_idle()
        .expect("sends, request, and reply settle");

    assert_eq!(
        runtime.state(&clerk).expect("clerk model"),
        &ClerkModel {
            total: Some(RequestOutcome::Replied(5)),
        }
    );
    let sends = runtime
        .trace()
        .iter()
        .filter(|record| {
            matches!(
                &record.event,
                TraceEvent::CommandEmitted {
                    kind: TraceCommandKind::Send,
                    target_port: Some(port),
                    target_component: None,
                    ..
                } if port == &PortId::new("ledger")
            )
        })
        .count();
    assert_eq!(
        sends, 2,
        "both sends are traced against the Port, not the provider"
    );
    assert!(runtime.cancel().expect("clean close").is_clean());
}

#[test]
fn a_port_request_to_times_out_under_the_usual_deadline_rules() {
    let patience = Duration::from_secs(1);
    let (program, _, clerk) = assemble(Some(patience));
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
        .build()
        .expect("nothing to control");

    runtime.send(&clerk, ClerkMessage::Start).expect("start");
    runtime
        .run_until_idle()
        .expect("the parked request is outstanding");
    assert_eq!(runtime.state(&clerk).expect("clerk model").total, None);
    runtime.advance(patience).expect("the deadline fires");
    assert_eq!(
        runtime.state(&clerk).expect("clerk model").total,
        Some(RequestOutcome::TimedOut)
    );
    assert!(runtime.cancel().expect("clean close").is_clean());
}

#[test]
fn into_request_to_inspects_a_request_without_a_runtime() {
    let mut program = Program::builder();
    let ledger = program.add_port::<LedgerMessage>(PortId::new("ledger"));

    let intent = Command::request(&ledger, LedgerMessage::Total)
        .producing(ClerkMessage::Total)
        .into_request::<LedgerMessage>()
        .ok()
        .expect("a request addressed with LedgerMessage");
    assert!(matches!(intent.message, LedgerMessage::Total(_)));
    assert!(matches!(
        intent.reply(RequestOutcome::Replied(5u64)),
        ClerkMessage::Total(RequestOutcome::Replied(5))
    ));

    // Anything else comes back unchanged.
    let send: Command<ClerkMessage> = Command::send(&ledger, LedgerMessage::Add(1));
    assert!(send.into_request::<LedgerMessage>().is_err());
    let other: Command<ClerkMessage> =
        Command::request(&ledger, LedgerMessage::Total).producing(ClerkMessage::Total);
    assert!(other.into_request::<ClerkMessage>().is_err());
}

#[test]
fn an_initial_send_to_a_foreign_port_fails_both_profile_builds() {
    struct Eager;
    impl Component for Eager {
        type Io = ();
        type Connections = ();
        // Held as configuration, which Samara does not look inside, so the
        // foreign Port is not caught as a declared connection at build.
        type Configuration = Port<LedgerMessage>;
        type Model = ();
        type Message = ClerkMessage;
        fn init(cx: &Context<Self>) -> Init<(), ClerkMessage> {
            Init::new(()).with_command(Command::send(&cx.config, LedgerMessage::Add(1)))
        }
        fn update(_: &Context<Self>, _: &mut (), _: ClerkMessage) -> Command<ClerkMessage> {
            Command::none()
        }
    }
    fn program() -> Program {
        let mut other = Program::builder();
        let foreign = other.add_port::<LedgerMessage>(PortId::new("ledger"));
        let mut program = Program::builder();
        program
            .configure_component::<Eager>()
            .name("eager")
            .config(foreign)
            .add(());
        program.build().expect("the hidden Port is ordinary Rust")
    }
    let live = Runtime::builder(program())
        .build()
        .err()
        .expect("live build fails");
    assert!(live.to_string().contains("another Program"), "{live}");
    let controlled = ControlledRuntime::builder(program())
        .control_all()
        .build()
        .err()
        .expect("controlled build fails");
    assert!(
        controlled.to_string().contains("another Program"),
        "{controlled}"
    );
}

#[tokio::test]
async fn a_port_handle_sends_and_requests_with_the_same_two_forms() -> Result<(), RuntimeError> {
    let (program, ledger, _) = assemble(None);
    let runtime = Runtime::builder(program).build()?;
    let host = runtime.port_handle(&ledger)?;
    let task = runtime.spawn();

    host.send(LedgerMessage::Add(4)).await?;
    host.send(LedgerMessage::Add(6)).await?;
    assert_eq!(host.request_to(LedgerMessage::Total).await?, 10);
    assert_eq!(
        host.request_to_timeout(LedgerMessage::Total, Duration::from_secs(5))
            .await?,
        RequestOutcome::Replied(10)
    );
    assert_eq!(
        host.request_to_timeout(LedgerMessage::Park, Duration::from_millis(10))
            .await?,
        RequestOutcome::TimedOut
    );

    assert!(task.shutdown(Shutdown::Cancel).await?.is_clean());
    assert!(host.send(LedgerMessage::Add(1)).await.is_err());
    Ok(())
}
