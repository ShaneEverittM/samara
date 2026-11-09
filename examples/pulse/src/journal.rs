use samara::prelude::*;

/// Where the recorder appends, relative to the root the host binds.
pub const JOURNAL_FILE: &str = "pulse.journal";

/// The journal's public vocabulary, carried by the `Port<JournalMessage>` the
/// Assembly exports.
#[derive(Debug)]
pub enum JournalMessage {
    /// Records one confirmed transition.
    Transition {
        name: String,
        from: String,
        to: String,
    },
    /// Asks for the most recent `count` recorded transitions.
    Recent {
        count: usize,
        reply_with: ReplyWith<Vec<String>>,
    },
}

/// Applies the hold policy and records confirmed transitions.
struct Recorder;

enum Message {
    Journal(JournalMessage),
}

enum Model {
    Idle,
}

/// The capabilities one Recorder is wired with.
struct RecorderConfig {
    append: EffectCapability<AppendToFile>,
    clock: EffectCapability<ReadSystemTime>,
}

impl Component for Recorder {
    type Config = RecorderConfig;
    type Model = Model;
    type Message = Message;

    fn init(_: &RecorderConfig) -> Init<Model, Message> {
        Init::new(Model::Idle)
    }

    fn update(config: &RecorderConfig, model: &mut Model, message: Message) -> Command<Message> {
        match message {
            Message::Journal(journal) => match (model, journal) {
                (Model::Idle, JournalMessage::Transition { name, from, to }) => {
                    let _ = (&config.append, &config.clock, name, from, to);
                    todo!()
                }
                (Model::Idle, JournalMessage::Recent { .. }) => todo!(),
            },
        }
    }
}

// A marker is the Assembly, so `Recorder`, its Model, and its
// Message stay private. Implementing it on the Component would make all three
// public, because `Component`'s associated types are part of its interface.
/// The Assembly's public name; its Connector accepts `JournalMessage`.
pub struct Journal;

impl Assembly for Journal {
    type Config = ();
    type Accepts = JournalMessage;
    type Exports = ();
    type Requires = (Effect<AppendToFile>, Effect<ReadSystemTime>);

    fn install(program: &mut ProgramBuilder, _: Self::Config) -> (Connector<JournalMessage>, ()) {
        let append = program.effect::<AppendToFile>();
        let clock = program.effect::<ReadSystemTime>();
        let recorder = program.component::<Recorder>(
            ComponentId::new("journal"),
            RecorderConfig { append, clock },
        );
        (recorder.accepting(Message::Journal), ())
    }
}

/// The drivers `pulse` vendors for the journal: the crate's filesystem driver
/// rooted at the working directory, and the system clock. Hosts may pass any
/// other pair that drives the same two effects to `and_drivers` instead.
impl VendorsDrivers for Journal {
    type Drivers = (FsDriver, SystemClockDriver);

    fn drivers() -> Self::Drivers {
        (FsDriver::new("."), SystemClockDriver)
    }
}
