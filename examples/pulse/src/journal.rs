use samara::prelude::*;

const JOURNAL: &str = "pulse/journal";

protocol! {
    pub type Journal => enum ProtocolMessage {
        Transition { name: String, from: String, to: String },
        Recent(usize) -> Vec<String>,
    }
}

enum Message {
    Protocol(ProtocolMessage),
}

impl From<ProtocolMessage> for Message {
    fn from(value: ProtocolMessage) -> Self {
        Self::Protocol(value)
    }
}

enum Model {
    Idle,
}

struct FsJournal;

impl Component for FsJournal {
    type Model = Model;
    type Message = Message;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(Model::Idle)
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            Message::Protocol(protocol) => match (model, protocol) {
                (Model::Idle, ProtocolMessage::Transition { name, from, to }) => {
                    todo!()
                }
                (Model::Idle, ProtocolMessage::Recent(_)) => todo!(),
            },
        }
    }
}

#[must_use = "bind the journal's drivers with Bindings::live or Bindings::controlled"]
pub struct Bindings {/* Private */}

pub fn install(program: &mut ProgramBuilder) -> (Port<Journal>, Bindings) {
    let journal = program.port::<Journal>(PortId::new(JOURNAL));
    let fs_journal = program.component(ComponentId::new("fs-journal"), FsJournal);
    program.bind_port(&journal, &fs_journal);

    (journal, Bindings {})
}
