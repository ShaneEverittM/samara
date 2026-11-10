use std::path::PathBuf;

use bytes::Bytes;
use samara::prelude::*;

/// Records confirmed transitions through the declared filesystem capability.
struct Recorder;

enum Message {
    Journal(JournalMessage),
    SendTailToClient {
        contents: Bytes,
        count: usize,
        reply: ReplyWith<Result<Vec<String>>>,
    },
    TellClientRecentFailed {
        error: EffectFailure<FsError>,
        reply: ReplyWith<Result<Vec<String>>>,
    },
}

#[derive(Default, Debug)]
struct Model;

/// Immutable logical settings forwarded from the Assembly.
struct RecorderConfig {
    file: PathBuf,
}

impl Component for Recorder {
    type Requires = (
        Effect<AppendToFile>,
        Effect<ReadSystemTime>,
        Effect<ReadFile>,
    );
    type Config = RecorderConfig;
    type Model = Model;
    type Message = Message;

    fn init(_: &RecorderConfig, _: &Capabilities<Self::Requires>) -> Init<Model, Message> {
        Init::default()
    }

    fn update(
        config: &RecorderConfig,
        (append, .., read): &Capabilities<Self::Requires>,
        _: &mut Model,
        message: Message,
    ) -> Command<Message> {
        match message {
            // Log the transition to the journal.
            Message::Journal(JournalMessage::Transition { name, from, to }) => {
                Command::effect_discarding_outcome(
                    append,
                    AppendToFile::line(&config.file, format!("{name}: {from} -> {to}")),
                )
            }

            // Dispatch a tail of the journal.
            Message::Journal(JournalMessage::Recent { count, reply }) => Command::effect_with(
                read,
                ReadFile::new(&config.file),
                move |result| match result {
                    EffectOutcome::Succeeded(contents) => Message::SendTailToClient {
                        contents,
                        count,
                        reply,
                    },
                    EffectOutcome::Failed(error) => {
                        Message::TellClientRecentFailed { error, reply }
                    }
                },
            ),

            // The tail request above completed, parse and give the results to the caller.
            Message::SendTailToClient {
                contents,
                count,
                reply,
            } => {
                let text = str::from_utf8(contents.as_ref()).expect("Invalid UTF-8");
                let mut lines: Vec<_> = text.lines().rev().take(count).map(String::from).collect();
                lines.reverse();

                Command::reply(reply, Ok(lines))
            }

            // The tail request failed.
            Message::TellClientRecentFailed { error, reply } => {
                let error = match error {
                    EffectFailure::Error(_) => Error::CouldNotReadJournal,
                    EffectFailure::Cancelled(_) => Error::JournalReadCanceled,
                };
                Command::reply(reply, Err(error))
            }
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Could not read from journal, effect failed")]
    CouldNotReadJournal,

    #[error("could not read from journal, canceled")]
    JournalReadCanceled,
}

// A marker is the Assembly, so `Recorder`, its Model, and its
// Message stay private. Implementing it on the Component would make all three
// public, because `Component`'s associated types are part of its interface.
/// The Assembly's public name; its Connector accepts `JournalMessage`.
pub struct Journal;

/// Logical Journal settings shared by live and controlled execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalConfig {
    /// User-supplied filename relative to the Driver's root.
    pub file: PathBuf,
}

/// Default filename, relative to the root the host supplies to the Driver.
pub const JOURNAL_FILE: &str = "pulse.journal";

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
    /// Reads up to `count` trailing lines from the returned file contents.
    ///
    /// Concurrent appends may or may not be reflected; an unterminated last
    /// line is included. Read errors and cancellation become Journal errors.
    Recent {
        count: usize,
        reply: ReplyWith<Result<Vec<String>>>,
    },
}

impl Assembly for Journal {
    type Config = JournalConfig;
    type Accepts = JournalMessage;
    type Exports = ();
    type Requires = (
        Effect<AppendToFile>,
        Effect<ReadSystemTime>,
        Effect<ReadFile>,
    );

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
