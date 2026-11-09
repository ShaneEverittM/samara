use samara::prelude::*;

/// Where the recorder appends, relative to the root the host binds.
pub const JOURNAL_FILE: &str = "pulse.journal";

/// The journal's public vocabulary: what the Assembly's `Connector` accepts.
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
        // Register the effects, so the Runtime is prepared
        // to service them, and so the user can supply drivers.
        let append = program.effect::<AppendToFile>();
        let clock = program.effect::<ReadSystemTime>();

        // Install the Component, so the Runtime will run it.
        let recorder = program.component::<Recorder>(
            ComponentId::new("journal"),
            RecorderConfig { append, clock },
        );

        // Wrap the Component with a Connector to hide the implementation.
        (recorder.accepting(Message::Journal), ())
    }
}

// In a "real" library, this would be behind feature flags and would
// transitively enable Samara features for filesystem and time.
// Additionally, an Assembly need not provide these if the user
// wants to ship their own. The only concern is good documentation
// on the contract of the Effects we use.
impl VendorsDrivers for Journal {
    type Drivers = (FsDriver, SystemClockDriver);

    fn vendor() -> Self::Drivers {
        (FsDriver::new("."), SystemClockDriver)
    }
}
