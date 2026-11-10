use bytes::Bytes;
use samara::prelude::*;

/// Records confirmed transitions through the declared filesystem capability.
struct Recorder;

enum Message {
    Journal(JournalMessage),
}

#[derive(Default, Debug)]
struct Model;

impl Component for Recorder {
    type Requires = (File<ReadAppend>, Effect<ReadSystemTime>);
    type Config = ();
    type Model = Model;
    type Message = Message;

    fn init(_: &(), _: &Capabilities<Self::Requires>) -> Init<Model, Message> {
        Init::default()
    }

    fn update(
        _: &(),
        (file, _clock): &Capabilities<Self::Requires>,
        _: &mut Model,
        message: Message,
    ) -> Command<Message> {
        match message {
            // Log the transition to the journal.
            Message::Journal(JournalMessage::Transition { name, from, to }) => {
                File::append_line(file, format!("{name}: {from} -> {to}")).discard_outcome()
            }

            // Answer with a tail of the journal, straight from the read's outcome.
            // The reply token rides in the continuation, so no private Message
            // variant is needed to carry it across the read.
            Message::Journal(JournalMessage::Recent { count, reply }) => File::read(file)
                .finish_with(move |outcome| {
                    Command::reply(
                        reply,
                        match outcome {
                            EffectOutcome::Succeeded(contents) => Ok(tail(&contents, count)),
                            EffectOutcome::Failed(_) => Err(Error::CouldNotReadJournal),
                        },
                    )
                }),
        }
    }
}

/// The last `count` lines of the journal's contents, oldest first.
fn tail(contents: &Bytes, count: usize) -> Vec<String> {
    let text = str::from_utf8(contents.as_ref()).expect("Invalid UTF-8");
    let mut lines: Vec<_> = text.lines().rev().take(count).map(String::from).collect();
    lines.reverse();
    lines
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Could not read from journal, effect failed")]
    CouldNotReadJournal,
}

// A marker is the Assembly, so `Recorder`, its Model, and its
// Message stay private. Implementing it on the Component would make all three
// public, because `Component`'s associated types are part of its interface.
/// The Assembly's public name; its Connector accepts `JournalMessage`.
pub struct Journal;

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
    type Config = ();
    type Exports = Connector<JournalMessage>;
    type Requires = (File<ReadAppend>, Effect<ReadSystemTime>);

    fn install<R: Requirements>(
        program: &mut ProgramBuilder<R>,
        _: Self::Config,
        capabilities: Capabilities<Self::Requires>,
    ) -> Result<Connector<JournalMessage>, ProgramBuildError> {
        let recorder = program
            .component::<Recorder>(())
            .with_capabilities(&capabilities)
            .finish()?;
        Ok(recorder.accepting(Message::Journal))
    }
}

#[cfg(test)]
mod tests;
