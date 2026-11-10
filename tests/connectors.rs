//! Acceptance evidence for `Connector`: a library Assembly
//! exports an erased provider instead of a Port of its own protocol, the
//! application declares the Port in its own vocabulary, and the coupling
//! between the two is written once, at the binding site. The library's
//! Component never becomes nameable outside its module.

use samara::prelude::*;

/// Stands in for a crate the application depends on. Everything but the
/// vocabulary and the Assembly is private.
mod journal_lib {
    use samara::prelude::*;

    /// The library's public vocabulary. A request carries its reply token.
    #[derive(Debug)]
    pub enum JournalMessage {
        Transition {
            name: String,
            from: String,
            to: String,
        },
        Recent(usize, ReplyWith<Vec<String>>),
    }

    struct Recorder;

    enum RecorderMessage {
        Journal(JournalMessage),
    }

    impl Component for Recorder {
        type Requires = ();
        type Config = ();
        type Model = Vec<String>;
        type Message = RecorderMessage;

        fn init(
            _: &(),
            _: &samara::Capabilities<Self::Requires>,
        ) -> Init<Vec<String>, RecorderMessage> {
            Init::new(Vec::new())
        }

        fn update(
            _: &(),
            _: &samara::Capabilities<Self::Requires>,
            entries: &mut Vec<String>,
            RecorderMessage::Journal(message): RecorderMessage,
        ) -> Command<RecorderMessage> {
            match message {
                JournalMessage::Transition { name, from, to } => {
                    entries.push(format!("{name}: {from} -> {to}"));
                    Command::none()
                }
                JournalMessage::Recent(count, reply_with) => {
                    let count = count.min(entries.len());
                    let recent = entries[entries.len() - count..].to_vec();
                    Command::reply(reply_with, recent)
                }
            }
        }
    }

    /// The Assembly's public name; it was the protocol marker before the message type became the Port's parameter.
    pub struct Journal;

    impl Assembly for Journal {
        type Config = ();
        type Accepts = JournalMessage;
        type Exports = ();
        type Requires = ();

        fn install<R: Requirements>(
            program: &mut ProgramBuilder<R>,
            _: (),
            _: Capabilities<Self::Requires>,
        ) -> Result<(Connector<JournalMessage>, ()), ProgramBuildError> {
            let recorder = program
                .component::<Recorder>(())
                .name("journal")
                .finish()
                .expect("component registration");
            Ok((recorder.accepting(RecorderMessage::Journal), ()))
        }
    }
}

use journal_lib::{Journal, JournalMessage};

// The application's own vocabulary. Its Components depend on this Port and
// never name `JournalMessage`.
#[derive(Debug)]
pub enum AuditMessage {
    Changed {
        what: String,
        from: String,
        to: String,
    },
    Tail(usize, ReplyWith<Vec<String>>),
}

struct Monitor;

struct MonitorConfig {
    audit: Port<AuditMessage>,
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
    type Requires = ();
    type Config = MonitorConfig;
    type Model = MonitorModel;
    type Message = MonitorMessage;

    fn init(
        _: &MonitorConfig,
        _: &samara::Capabilities<Self::Requires>,
    ) -> Init<MonitorModel, MonitorMessage> {
        Init::new(MonitorModel::default())
    }

    fn update(
        config: &MonitorConfig,
        _: &samara::Capabilities<Self::Requires>,
        model: &mut MonitorModel,
        message: MonitorMessage,
    ) -> Command<MonitorMessage> {
        match message {
            MonitorMessage::Start => Command::batch([
                Command::send(
                    &config.audit,
                    AuditMessage::Changed {
                        what: "api".into(),
                        from: "up".into(),
                        to: "down".into(),
                    },
                ),
                Command::send(
                    &config.audit,
                    AuditMessage::Changed {
                        what: "api".into(),
                        from: "down".into(),
                        to: "up".into(),
                    },
                ),
                Command::request_to(
                    &config.audit,
                    |reply_with| AuditMessage::Tail(1, reply_with),
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

/// Couples the application's vocabulary to the library's. The only place
/// `JournalMessage` is spelled outside the library.
fn couple(message: AuditMessage) -> JournalMessage {
    match message {
        AuditMessage::Changed { what, from, to } => JournalMessage::Transition {
            name: what,
            from,
            to,
        },
        AuditMessage::Tail(count, reply_with) => JournalMessage::Recent(count, reply_with),
    }
}

fn assemble() -> (Program, InstalledAssembly<Journal>, ComponentRef<Monitor>) {
    let (mut program, ()) = samara::Program::builder::<()>();
    let audit = program.port::<AuditMessage>(PortId::new("audit"));
    let journal = program
        .install::<Journal>(())
        .finish()
        .expect("assembly installation");
    program.connect(&audit, &journal.connector().accepting(couple));
    let monitor = program
        .component::<Monitor>(MonitorConfig { audit })
        .name("monitor")
        .finish()
        .expect("component registration");
    (
        program.build().expect("program assembles"),
        journal,
        monitor,
    )
}

#[test]
fn an_application_port_reaches_a_library_connector_through_one_coupling() {
    let (program, _journal, monitor) = assemble();
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
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
fn connectors_are_inert_addresses_that_remember_only_what_they_accept() {
    let (mut program, ()) = samara::Program::builder::<()>();
    let journal = program
        .install::<Journal>(())
        .finish()
        .expect("assembly installation");
    let connector: &Connector<JournalMessage> = journal.connector();
    assert_eq!(connector.id().name(), "journal");
    let shown = format!("{connector:?}");
    assert!(shown.contains("JournalMessage"), "{shown}");
    assert!(!shown.contains("Recorder"), "{shown}");

    // A ComponentRef erases to a connector of its own message type through
    // `connector()`, which is what `connect` takes.
    let audit = program.port::<AuditMessage>(PortId::new("audit"));
    let monitor = program
        .component::<Monitor>(MonitorConfig { audit })
        .name("monitor")
        .finish()
        .expect("component registration");
    let same: Connector<MonitorMessage> = monitor.connector();
    assert_eq!(same.id(), monitor.id());
}

#[test]
fn a_connector_from_another_program_is_rejected_at_build() {
    let (mut other, ()) = samara::Program::builder::<()>();
    let foreign = other
        .install::<Journal>(())
        .finish()
        .expect("assembly installation");

    let (mut program, ()) = samara::Program::builder::<()>();
    let audit = program.port::<AuditMessage>(PortId::new("audit"));
    program.connect(&audit, &foreign.connector().accepting(couple));
    assert!(
        program.build().is_err(),
        "a foreign connector is not a provider here"
    );
}

// ---------------------------------------------------------------------------
// A Connector as a first-class address: send and request with no Port and no
// message enum, the way an Elm module exposes an opaque `Msg` with constructors.
// ---------------------------------------------------------------------------

/// Stands in for a crate written the Elm way: an opaque message type whose
/// public surface is its constructor functions. No shared enum, no Port.
mod tally_lib {
    use samara::prelude::*;

    pub struct TallyMessage(Inner);

    enum Inner {
        Add(u64),
        Total(ReplyWith<u64>),
        Park(ReplyWith<u64>),
    }

    impl TallyMessage {
        pub fn add(amount: u64) -> Self {
            Self(Inner::Add(amount))
        }

        /// A request: the `ReplyWith<u64>` parameter is what types the reply.
        pub fn total(reply_with: ReplyWith<u64>) -> Self {
            Self(Inner::Total(reply_with))
        }

        /// A request the provider accepts and never answers.
        pub fn park(reply_with: ReplyWith<u64>) -> Self {
            Self(Inner::Park(reply_with))
        }
    }

    struct Counter;

    struct CounterModel {
        total: u64,
        parked: Vec<ReplyWith<u64>>,
    }

    impl Component for Counter {
        type Requires = ();
        type Config = ();
        type Model = CounterModel;
        type Message = TallyMessage;

        fn init(
            _: &(),
            _: &samara::Capabilities<Self::Requires>,
        ) -> Init<CounterModel, TallyMessage> {
            Init::new(CounterModel {
                total: 0,
                parked: Vec::new(),
            })
        }

        fn update(
            _: &(),
            _: &samara::Capabilities<Self::Requires>,
            model: &mut CounterModel,
            TallyMessage(inner): TallyMessage,
        ) -> Command<TallyMessage> {
            match inner {
                Inner::Add(amount) => {
                    model.total += amount;
                    Command::none()
                }
                Inner::Total(reply_with) => Command::reply(reply_with, model.total),
                Inner::Park(reply_with) => {
                    model.parked.push(reply_with);
                    Command::none()
                }
            }
        }
    }

    pub struct Tally;

    impl Assembly for Tally {
        type Config = ();
        type Accepts = TallyMessage;
        type Exports = ();
        type Requires = ();

        fn install<R: Requirements>(
            program: &mut ProgramBuilder<R>,
            _: (),
            _: Capabilities<Self::Requires>,
        ) -> Result<(Connector<TallyMessage>, ()), ProgramBuildError> {
            let counter = program
                .component::<Counter>(())
                .name("tally")
                .finish()
                .expect("component registration");
            Ok((counter.connector(), ()))
        }
    }
}

use std::time::Duration;
use tally_lib::{Tally, TallyMessage};

struct Teller;

struct TellerConfig {
    tally: Connector<TallyMessage>,
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
    type Requires = ();
    type Config = TellerConfig;
    type Model = TellerModel;
    type Message = TellerMessage;

    fn init(
        _: &TellerConfig,
        _: &samara::Capabilities<Self::Requires>,
    ) -> Init<TellerModel, TellerMessage> {
        Init::new(TellerModel::default())
    }

    fn update(
        config: &TellerConfig,
        _: &samara::Capabilities<Self::Requires>,
        model: &mut TellerModel,
        message: TellerMessage,
    ) -> Command<TellerMessage> {
        match message {
            TellerMessage::Start => Command::batch([
                Command::send(&config.tally, TallyMessage::add(2)),
                Command::send(&config.tally, TallyMessage::add(3)),
                match config.patience {
                    None => Command::request_to(
                        &config.tally,
                        TallyMessage::total,
                        TellerMessage::Total,
                    ),
                    Some(patience) => Command::request_to_timeout(
                        &config.tally,
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

fn assemble_tally(
    patience: Option<Duration>,
) -> (Program, InstalledAssembly<Tally>, ComponentRef<Teller>) {
    let (mut program, ()) = samara::Program::builder::<()>();
    let tally = program
        .install::<Tally>(())
        .finish()
        .expect("assembly installation");
    let teller = program
        .component::<Teller>(TellerConfig {
            tally: tally.connector().clone(),
            patience,
        })
        .name("teller")
        .finish()
        .expect("component registration");
    (program.build().expect("program assembles"), tally, teller)
}

#[test]
fn a_component_sends_and_requests_through_a_connector_with_no_port_or_protocol() {
    let (program, tally, teller) = assemble_tally(None);
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
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
            target_port: None,
            ..
        } if target == tally.connector().id()
    ));
    assert!(runtime.cancel().expect("clean close").is_clean());
}

#[test]
fn a_connector_request_times_out_exactly_like_a_port_request() {
    let patience = Duration::from_secs(1);
    let (program, _tally, teller) = assemble_tally(Some(patience));
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
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

/// Holds an adapted connector in its own vocabulary and never names the library's.
struct DirectMonitor;

struct DirectMonitorConfig {
    audit: Connector<AuditMessage>,
}

impl Component for DirectMonitor {
    type Requires = ();
    type Config = DirectMonitorConfig;
    type Model = MonitorModel;
    type Message = MonitorMessage;

    fn init(
        _: &DirectMonitorConfig,
        _: &samara::Capabilities<Self::Requires>,
    ) -> Init<MonitorModel, MonitorMessage> {
        Init::new(MonitorModel::default())
    }

    fn update(
        config: &DirectMonitorConfig,
        _: &samara::Capabilities<Self::Requires>,
        model: &mut MonitorModel,
        message: MonitorMessage,
    ) -> Command<MonitorMessage> {
        match message {
            MonitorMessage::Start => Command::batch([
                Command::send(
                    &config.audit,
                    AuditMessage::Changed {
                        what: "db".into(),
                        from: "cold".into(),
                        to: "warm".into(),
                    },
                ),
                Command::request_to(
                    &config.audit,
                    |reply_with| AuditMessage::Tail(1, reply_with),
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
fn a_connector_is_adapted_to_the_holders_vocabulary_once_with_accepting() {
    let (mut program, ()) = samara::Program::builder::<()>();
    let journal = program
        .install::<Journal>(())
        .finish()
        .expect("assembly installation");
    // Elm 0.16's `Signal.forwardTo`: the library's connector, speaking our language.
    let audit: Connector<AuditMessage> = journal.connector().accepting(couple);
    let shown = format!("{audit:?}");
    assert!(
        shown.contains("AuditMessage") && !shown.contains("JournalMessage"),
        "{shown}"
    );
    let monitor = program
        .component::<DirectMonitor>(DirectMonitorConfig { audit })
        .name("monitor")
        .finish()
        .expect("component registration");
    let program = program.build().expect("program assembles");
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
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

/// Issues a request to whatever connector it is given before anything runs.
struct EagerTeller;

impl Component for EagerTeller {
    type Requires = ();
    type Config = Connector<TallyMessage>;
    type Model = ();
    type Message = TellerMessage;

    fn init(
        tally: &Connector<TallyMessage>,
        _: &samara::Capabilities<Self::Requires>,
    ) -> Init<(), TellerMessage> {
        Init::new(()).with_command(Command::request_to(
            tally,
            TallyMessage::total,
            TellerMessage::Total,
        ))
    }

    fn update(
        _: &Connector<TallyMessage>,
        _: &samara::Capabilities<Self::Requires>,
        _: &mut (),
        _: TellerMessage,
    ) -> Command<TellerMessage> {
        Command::none()
    }
}

#[test]
fn an_initial_request_to_a_foreign_connector_fails_both_profile_builds() {
    fn program() -> Program {
        let (mut other, ()) = samara::Program::builder::<()>();
        let foreign = other
            .install::<Tally>(())
            .finish()
            .expect("assembly installation");
        let (mut program, ()) = samara::Program::builder::<()>();
        program
            .component::<EagerTeller>(foreign.connector().clone())
            .name("eager")
            .finish()
            .expect("component registration");
        program
            .build()
            .expect("the hidden connector is ordinary Rust until a profile reads initial work")
    }

    let live = Runtime::builder(program())
        .build()
        .err()
        .expect("initial foreign connector must fail live build");
    assert!(live.to_string().contains("another Program"), "{live}");

    let controlled = ControlledRuntime::builder(program())
        .control_all()
        .build()
        .err()
        .expect("initial foreign connector must fail controlled build");
    assert!(
        controlled.to_string().contains("another Program"),
        "{controlled}"
    );
}

#[test]
fn a_connector_request_is_inspectable_without_a_runtime() {
    let (mut program, ()) = samara::Program::builder::<()>();
    let tally = program
        .install::<Tally>(())
        .finish()
        .expect("assembly installation");
    let command: Command<TellerMessage> =
        Command::request_to(tally.connector(), TallyMessage::total, TellerMessage::Total);
    // The message is built with an inert reply token; the continuation runs on
    // an outcome the test chooses. The library's private `Inner` stays private,
    // so the test can only observe what the constructor exposes.
    let intent = command
        .into_request_to::<TallyMessage>()
        .ok()
        .expect("a request addressed with TallyMessage");
    assert!(matches!(
        intent.reply(RequestOutcome::Replied(5u64)),
        TellerMessage::Total(RequestOutcome::Replied(5))
    ));
}
