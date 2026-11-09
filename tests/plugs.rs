//! Acceptance evidence for `Plug` and `Pluggable`: a library Assembly
//! exports an erased provider instead of a Jack of its own protocol, the
//! application declares the Jack in its own vocabulary, and the coupling
//! between the two is written once, at the binding site. The library's
//! Component never becomes nameable outside its module.

use samara::prelude::*;

/// Stands in for a crate the application depends on. Everything but the
/// vocabulary and the Assembly is private.
mod journal_lib {
    use samara::prelude::*;

    protocol! {
        pub type Journal => enum JournalMessage {
            Transition { name: String, from: String, to: String },
            Recent(usize) -> Vec<String>,
        }
    }

    struct Recorder;

    enum RecorderMessage {
        Journal(JournalMessage),
    }

    impl Component for Recorder {
        type Config = ();
        type Model = Vec<String>;
        type Message = RecorderMessage;

        fn init(_: &()) -> Init<Vec<String>, RecorderMessage> {
            Init::new(Vec::new())
        }

        fn update(
            _: &(),
            entries: &mut Vec<String>,
            RecorderMessage::Journal(message): RecorderMessage,
        ) -> Command<RecorderMessage> {
            match message {
                JournalMessage::Transition { name, from, to } => {
                    entries.push(format!("{name}: {from} -> {to}"));
                    Command::none()
                }
                JournalMessage::Recent(call) => {
                    let count = call.request.0.min(entries.len());
                    let recent = entries[entries.len() - count..].to_vec();
                    Command::reply(call.reply_to, recent)
                }
            }
        }
    }

    impl Assembly for Journal {
        type Config = ();
        type Exports = Plug<JournalMessage>;
        type Requires = ();

        fn install(program: &mut ProgramBuilder, _: ()) -> Plug<JournalMessage> {
            program
                .component::<Recorder>(ComponentId::new("journal"), ())
                .plug_with(RecorderMessage::Journal)
        }
    }
}

use journal_lib::{Journal, JournalMessage, Recent};

// The application's own vocabulary. Its Components depend on this Jack and
// never name `JournalMessage`.
protocol! {
    pub type Audit => enum AuditMessage {
        Changed { what: String, from: String, to: String },
        Tail(usize) -> Vec<String>,
    }
}

struct Monitor;

struct MonitorConfig {
    audit: Jack<Audit>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct MonitorModel {
    tail: Option<Vec<String>>,
}

enum MonitorMessage {
    Start,
    Tail(RequestOutcome<Vec<String>>),
}

impl Component for Monitor {
    type Config = MonitorConfig;
    type Model = MonitorModel;
    type Message = MonitorMessage;

    fn init(_: &MonitorConfig) -> Init<MonitorModel, MonitorMessage> {
        Init::new(MonitorModel::default())
    }

    fn update(
        config: &MonitorConfig,
        model: &mut MonitorModel,
        message: MonitorMessage,
    ) -> Command<MonitorMessage> {
        match message {
            MonitorMessage::Start => Command::batch([
                Command::notify(
                    config.audit.clone(),
                    Changed {
                        what: "api".into(),
                        from: "up".into(),
                        to: "down".into(),
                    },
                ),
                Command::notify(
                    config.audit.clone(),
                    Changed {
                        what: "api".into(),
                        from: "down".into(),
                        to: "up".into(),
                    },
                ),
                Command::request_with(config.audit.clone(), Tail(1), MonitorMessage::Tail),
            ]),
            MonitorMessage::Tail(RequestOutcome::Replied(tail)) => {
                model.tail = Some(tail);
                Command::none()
            }
            MonitorMessage::Tail(_) => Command::none(),
        }
    }
}

/// Couples the application's vocabulary to the library's. The only place
/// `JournalMessage` is spelled outside the library.
fn couple(message: AuditMessage) -> JournalMessage {
    match message {
        AuditMessage::Changed { what, from, to } => JournalMessage::Transition {
            name: what,
            from,
            to,
        },
        AuditMessage::Tail(call) => JournalMessage::Recent(RequestInvocation::new(
            Recent(call.request.0),
            call.reply_to,
        )),
    }
}

fn assemble() -> (Program, Installed<Journal>, ComponentRef<Monitor>) {
    let mut program = Program::builder();
    let audit = program.jack::<Audit>(JackId::new("audit"));
    let journal = program.install::<Journal>(());
    program.plug_in_with(&audit, journal.exports(), couple);
    let monitor =
        program.component::<Monitor>(ComponentId::new("monitor"), MonitorConfig { audit });
    (
        program.build().expect("program assembles"),
        journal,
        monitor,
    )
}

#[test]
fn an_application_jack_reaches_a_library_plug_through_one_coupling() {
    let (program, journal, monitor) = assemble();
    let mut runtime = ControlledRuntime::builder(program)
        .with_assembly(journal)
        .build()
        .expect("the library requires nothing");

    runtime
        .send(&monitor, MonitorMessage::Start)
        .expect("start");
    runtime
        .run_until_idle()
        .expect("notifications, request, and reply settle");

    assert_eq!(
        runtime.state(&monitor).expect("monitor model"),
        &MonitorModel {
            tail: Some(vec!["api: down -> up".into()]),
        }
    );
    assert!(runtime.cancel().expect("clean close").is_clean());
}

#[test]
fn plugs_are_inert_addresses_that_remember_only_what_they_accept() {
    let mut program = Program::builder();
    let journal = program.install::<Journal>(());
    let plug: &Plug<JournalMessage> = journal.exports();
    assert_eq!(plug.id(), &ComponentId::new("journal"));
    let shown = format!("{plug:?}");
    assert!(shown.contains("JournalMessage"), "{shown}");
    assert!(!shown.contains("Recorder"), "{shown}");

    // A ComponentRef erases to a plug of its own message type through
    // `Pluggable`, and `plug_in` accepts either form.
    let audit = program.jack::<Audit>(JackId::new("audit"));
    let monitor =
        program.component::<Monitor>(ComponentId::new("monitor"), MonitorConfig { audit });
    let same: Plug<MonitorMessage> = monitor.to_plug();
    assert_eq!(same.id(), monitor.id());
}

#[test]
fn a_plug_from_another_program_is_rejected_at_build() {
    let mut other = Program::builder();
    let foreign = other.install::<Journal>(());

    let mut program = Program::builder();
    let audit = program.jack::<Audit>(JackId::new("audit"));
    program.plug_in_with(&audit, foreign.exports(), couple);
    assert!(
        program.build().is_err(),
        "a foreign plug is not a provider here"
    );
}

// ---------------------------------------------------------------------------
// A Plug as a first-class address: send and request with no Jack and no
// Protocol, the way an Elm module exposes an opaque `Msg` with constructors.
// ---------------------------------------------------------------------------

/// Stands in for a crate written the Elm way: an opaque message type whose
/// public surface is its constructor functions. No `protocol!`, no Jack.
mod tally_lib {
    use samara::prelude::*;

    pub struct TallyMessage(Inner);

    enum Inner {
        Add(u64),
        Total(ReplyTo<u64>),
        Park(ReplyTo<u64>),
    }

    impl TallyMessage {
        pub fn add(amount: u64) -> Self {
            Self(Inner::Add(amount))
        }

        /// A request: the `ReplyTo<u64>` parameter is what types the reply.
        pub fn total(reply_to: ReplyTo<u64>) -> Self {
            Self(Inner::Total(reply_to))
        }

        /// A request the provider accepts and never answers.
        pub fn park(reply_to: ReplyTo<u64>) -> Self {
            Self(Inner::Park(reply_to))
        }
    }

    struct Counter;

    struct CounterModel {
        total: u64,
        parked: Vec<ReplyTo<u64>>,
    }

    impl Component for Counter {
        type Config = ();
        type Model = CounterModel;
        type Message = TallyMessage;

        fn init(_: &()) -> Init<CounterModel, TallyMessage> {
            Init::new(CounterModel {
                total: 0,
                parked: Vec::new(),
            })
        }

        fn update(
            _: &(),
            model: &mut CounterModel,
            TallyMessage(inner): TallyMessage,
        ) -> Command<TallyMessage> {
            match inner {
                Inner::Add(amount) => {
                    model.total += amount;
                    Command::none()
                }
                Inner::Total(reply_to) => Command::reply(reply_to, model.total),
                Inner::Park(reply_to) => {
                    model.parked.push(reply_to);
                    Command::none()
                }
            }
        }
    }

    pub struct Tally;

    impl Assembly for Tally {
        type Config = ();
        type Exports = Plug<TallyMessage>;
        type Requires = ();

        fn install(program: &mut ProgramBuilder, _: ()) -> Plug<TallyMessage> {
            program
                .component::<Counter>(ComponentId::new("tally"), ())
                .to_plug()
        }
    }
}

use std::time::Duration;
use tally_lib::{Tally, TallyMessage};

struct Teller;

struct TellerConfig {
    tally: Plug<TallyMessage>,
    /// `None` asks for the total; `Some` parks a request with that timeout.
    patience: Option<Duration>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct TellerModel {
    total: Option<RequestOutcome<u64>>,
}

enum TellerMessage {
    Start,
    Total(RequestOutcome<u64>),
}

impl Component for Teller {
    type Config = TellerConfig;
    type Model = TellerModel;
    type Message = TellerMessage;

    fn init(_: &TellerConfig) -> Init<TellerModel, TellerMessage> {
        Init::new(TellerModel::default())
    }

    fn update(
        config: &TellerConfig,
        model: &mut TellerModel,
        message: TellerMessage,
    ) -> Command<TellerMessage> {
        match message {
            TellerMessage::Start => Command::batch([
                Command::send(config.tally.clone(), TallyMessage::add(2)),
                Command::send(config.tally.clone(), TallyMessage::add(3)),
                match config.patience {
                    None => Command::request_to(
                        config.tally.clone(),
                        TallyMessage::total,
                        TellerMessage::Total,
                    ),
                    Some(patience) => Command::request_to_timeout(
                        config.tally.clone(),
                        TallyMessage::park,
                        patience,
                        TellerMessage::Total,
                    ),
                },
            ]),
            TellerMessage::Total(outcome) => {
                model.total = Some(outcome);
                Command::none()
            }
        }
    }
}

fn assemble_tally(patience: Option<Duration>) -> (Program, Installed<Tally>, ComponentRef<Teller>) {
    let mut program = Program::builder();
    let tally = program.install::<Tally>(());
    let teller = program.component::<Teller>(
        ComponentId::new("teller"),
        TellerConfig {
            tally: tally.exports().clone(),
            patience,
        },
    );
    (program.build().expect("program assembles"), tally, teller)
}

#[test]
fn a_component_sends_and_requests_through_a_plug_with_no_jack_or_protocol() {
    let (program, tally, teller) = assemble_tally(None);
    let mut runtime = ControlledRuntime::builder(program)
        .with_assembly(tally)
        .build()
        .expect("the library requires nothing");

    runtime.send(&teller, TellerMessage::Start).expect("start");
    runtime
        .run_until_idle()
        .expect("sends, request, and reply settle");

    assert_eq!(
        runtime.state(&teller).expect("teller model"),
        &TellerModel {
            total: Some(RequestOutcome::Replied(5)),
        }
    );
    let request = runtime
        .trace()
        .iter()
        .find(|record| {
            matches!(
                record.event,
                TraceEvent::CommandEmitted {
                    kind: TraceCommandKind::Request,
                    ..
                }
            )
        })
        .expect("the request is traced");
    assert!(matches!(
        &request.event,
        TraceEvent::CommandEmitted {
            target_component: Some(target),
            target_jack: None,
            ..
        } if target == &ComponentId::new("tally")
    ));
    assert!(runtime.cancel().expect("clean close").is_clean());
}

#[test]
fn a_plug_request_times_out_exactly_like_a_jack_request() {
    let patience = Duration::from_secs(1);
    let (program, tally, teller) = assemble_tally(Some(patience));
    let mut runtime = ControlledRuntime::builder(program)
        .with_assembly(tally)
        .build()
        .expect("the library requires nothing");

    runtime.send(&teller, TellerMessage::Start).expect("start");
    runtime
        .run_until_idle()
        .expect("the parked request is outstanding");
    assert_eq!(runtime.state(&teller).expect("teller model").total, None);

    runtime.advance(patience).expect("the deadline fires");
    assert_eq!(
        runtime.state(&teller).expect("teller model").total,
        Some(RequestOutcome::TimedOut)
    );
    assert!(runtime.cancel().expect("clean close").is_clean());
}

/// Holds an adapted plug in its own vocabulary and never names the library's.
struct DirectMonitor;

struct DirectMonitorConfig {
    audit: Plug<AuditMessage>,
}

impl Component for DirectMonitor {
    type Config = DirectMonitorConfig;
    type Model = MonitorModel;
    type Message = MonitorMessage;

    fn init(_: &DirectMonitorConfig) -> Init<MonitorModel, MonitorMessage> {
        Init::new(MonitorModel::default())
    }

    fn update(
        config: &DirectMonitorConfig,
        model: &mut MonitorModel,
        message: MonitorMessage,
    ) -> Command<MonitorMessage> {
        match message {
            MonitorMessage::Start => Command::batch([
                Command::send(
                    config.audit.clone(),
                    AuditMessage::Changed {
                        what: "db".into(),
                        from: "cold".into(),
                        to: "warm".into(),
                    },
                ),
                Command::request_to(
                    config.audit.clone(),
                    |reply_to| AuditMessage::Tail(RequestInvocation::new(Tail(1), reply_to)),
                    MonitorMessage::Tail,
                ),
            ]),
            MonitorMessage::Tail(RequestOutcome::Replied(tail)) => {
                model.tail = Some(tail);
                Command::none()
            }
            MonitorMessage::Tail(_) => Command::none(),
        }
    }
}

#[test]
fn a_plug_is_adapted_to_the_holders_vocabulary_once_with_accepting() {
    let mut program = Program::builder();
    let journal = program.install::<Journal>(());
    // Elm 0.16's `Signal.forwardTo`: the library's plug, speaking our language.
    let audit: Plug<AuditMessage> = journal.exports().accepting(couple);
    let shown = format!("{audit:?}");
    assert!(
        shown.contains("AuditMessage") && !shown.contains("JournalMessage"),
        "{shown}"
    );
    let monitor = program
        .component::<DirectMonitor>(ComponentId::new("monitor"), DirectMonitorConfig { audit });
    let program = program.build().expect("program assembles");
    let mut runtime = ControlledRuntime::builder(program)
        .with_assembly(journal)
        .build()
        .expect("the library requires nothing");

    runtime
        .send(&monitor, MonitorMessage::Start)
        .expect("start");
    runtime
        .run_until_idle()
        .expect("send, request, and reply settle");
    assert_eq!(
        runtime.state(&monitor).expect("monitor model"),
        &MonitorModel {
            tail: Some(vec!["db: cold -> warm".into()]),
        }
    );
    assert!(runtime.cancel().expect("clean close").is_clean());
}

/// Issues a request to whatever plug it is given before anything runs.
struct EagerTeller;

impl Component for EagerTeller {
    type Config = Plug<TallyMessage>;
    type Model = ();
    type Message = TellerMessage;

    fn init(tally: &Plug<TallyMessage>) -> Init<(), TellerMessage> {
        Init::new(()).with_command(Command::request_to(
            tally.clone(),
            TallyMessage::total,
            TellerMessage::Total,
        ))
    }

    fn update(_: &Plug<TallyMessage>, _: &mut (), _: TellerMessage) -> Command<TellerMessage> {
        Command::none()
    }
}

#[test]
fn an_initial_request_to_a_foreign_plug_fails_both_profile_builds() {
    fn program() -> Program {
        let mut other = Program::builder();
        let foreign = other.install::<Tally>(());
        let mut program = Program::builder();
        program.component::<EagerTeller>(ComponentId::new("eager"), foreign.exports().clone());
        program
            .build()
            .expect("the hidden plug is ordinary Rust until a profile reads initial work")
    }

    let live = LiveRuntime::builder(program())
        .build()
        .err()
        .expect("initial foreign plug must fail live build");
    assert!(live.to_string().contains("another Program"), "{live}");

    let controlled = ControlledRuntime::builder(program())
        .build()
        .err()
        .expect("initial foreign plug must fail controlled build");
    assert!(
        controlled.to_string().contains("another Program"),
        "{controlled}"
    );
}

#[test]
fn request_intents_only_see_jack_requests() {
    let mut program = Program::builder();
    let tally = program.install::<Tally>(());
    let command: Command<TellerMessage> = Command::request_to(
        tally.exports().clone(),
        TallyMessage::total,
        TellerMessage::Total,
    );
    // A Plug request has no request value until the runtime mints its token;
    // it is observed through controlled execution instead.
    assert!(command.request_intents::<()>().is_empty());
    assert!(command.request_intents::<TallyMessage>().is_empty());
}
