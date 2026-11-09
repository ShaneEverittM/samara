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
