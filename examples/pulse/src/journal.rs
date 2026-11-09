use std::path::PathBuf;

use samara::prelude::*;

/// Default filename, relative to the root the host supplies to the Driver.
pub const JOURNAL_FILE: &str = "pulse.journal";

/// Logical Journal settings shared by live and controlled execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalConfig {
    /// User-supplied filename relative to the Driver's root.
    pub file: PathBuf,
}

impl Default for JournalConfig {
    fn default() -> Self {
        Self {
            file: JOURNAL_FILE.into(),
        }
    }
}

/// The journal's public vocabulary, accepted by the Assembly's Connector.
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

/// Records confirmed transitions through the declared filesystem capability.
struct Recorder;

enum Message {
    Journal(JournalMessage),
    Appended(EffectOutcome<(), FsError>),
}

#[derive(Default)]
struct Model {
    /// The most recently completed append; pending work does not overwrite it.
    last_append: Option<EffectOutcome<(), FsError>>,
}

/// Immutable logical settings forwarded from the Assembly.
struct RecorderConfig {
    file: PathBuf,
}

impl Component for Recorder {
    type Requires = (Effect<AppendToFile>, Effect<ReadSystemTime>);
    type Config = RecorderConfig;
    type Model = Model;
    type Message = Message;

    fn init(_: &RecorderConfig, _: &Capabilities<Self::Requires>) -> Init<Model, Message> {
        Init::new(Model::default())
    }

    fn update(
        config: &RecorderConfig,
        (append, _clock): &Capabilities<Self::Requires>,
        model: &mut Model,
        message: Message,
    ) -> Command<Message> {
        match message {
            Message::Journal(JournalMessage::Transition { name, from, to }) => {
                Command::effect_with(
                    append,
                    AppendToFile::line(config.file.clone(), format!("{name}: {from} -> {to}")),
                    Message::Appended,
                )
            }
            Message::Journal(JournalMessage::Recent { .. }) => todo!(),
            Message::Appended(outcome) => {
                model.last_append = Some(outcome);
                Command::none()
            }
        }
    }
}

// A marker is the Assembly, so `Recorder`, its Model, and its
// Message stay private. Implementing it on the Component would make all three
// public, because `Component`'s associated types are part of its interface.
/// The Assembly's public name; its Connector accepts `JournalMessage`.
pub struct Journal;

impl Assembly for Journal {
    type Config = JournalConfig;
    type Accepts = JournalMessage;
    type Exports = ();
    type Requires = (Effect<AppendToFile>, Effect<ReadSystemTime>);

    fn install<R: Requirements>(
        program: &mut ProgramBuilder<R>,
        config: Self::Config,
        capabilities: Capabilities<Self::Requires>,
    ) -> Result<(Connector<JournalMessage>, ()), ProgramBuildError> {
        let recorder = program
            .component::<Recorder>(
                ComponentId::new("journal"),
                RecorderConfig { file: config.file },
            )
            .with_capabilities(capabilities)
            .finish()?;
        Ok((recorder.accepting(Message::Journal), ()))
    }
}

#[cfg(test)]
mod tests;
