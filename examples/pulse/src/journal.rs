use samara::prelude::*;

const JOURNAL: &str = "pulse/journal";

/// Where the recorder appends, relative to the root the host binds.
pub const JOURNAL_FILE: &str = "pulse.journal";

messages! {
    pub enum ProtocolMessage {
        Transition { name: String, from: String, to: String },
        Recent(usize) -> Vec<String>,
    }
}

/// Applies the hold policy and records confirmed transitions.
struct Recorder;

enum Message {
    Protocol(ProtocolMessage),
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
            Message::Protocol(protocol) => match (model, protocol) {
                (Model::Idle, ProtocolMessage::Transition { name, from, to }) => {
                    let _ = (&config.append, &config.clock, name, from, to);
                    todo!()
                }
                (Model::Idle, ProtocolMessage::Recent(_)) => todo!(),
            },
        }
    }
}

// A marker is the Assembly, so `Recorder`, its Model, and its
// Message stay private. Implementing it on the Component would make all three
// public, because `Component`'s associated types are part of its interface.
/// The Assembly's public name; it was the protocol marker before the message type became the Port's parameter.
pub struct Journal;

impl Assembly for Journal {
    type Config = ();
    type Exports = Port<ProtocolMessage>;
    type Requires = (Effect<AppendToFile>, Effect<ReadSystemTime>);

    fn install(program: &mut ProgramBuilder, _: Self::Config) -> Port<ProtocolMessage> {
        let append = program.effect::<AppendToFile>();
        let clock = program.effect::<ReadSystemTime>();
        let journal = program.port::<ProtocolMessage>(PortId::new(JOURNAL));
        let recorder = program.component::<Recorder>(
            ComponentId::new("journal"),
            RecorderConfig { append, clock },
        );
        program.connect_with(&journal, &recorder, Message::Protocol);

        journal
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
