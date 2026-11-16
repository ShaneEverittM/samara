use bytes::Bytes;
use samara::prelude::*;
use std::time::{SystemTime, UNIX_EPOCH};

/// Everything the journal can reach: its file and the clock.
#[derive(Clone, Capabilities)]
pub struct JournalIo {
    pub file: File<ReadAppend>,
    pub clock: Effect<ReadSystemTime>,
}

/// Records confirmed transitions through its file.
struct Recorder;

enum Message {
    Journal(JournalMessage),
    Time {
        time: SystemTime,
        transition: Transition,
    },
}

#[derive(Default, Debug)]
struct Model;

impl Component for Recorder {
    type Io = JournalIo;
    type Connections = ();
    type Configuration = ();
    type Model = Model;
    type Message = Message;

    fn init(_: &Context<Self>) -> Init<Model, Message> {
        Init::default()
    }

    fn update(cx: &Context<Self>, _: &mut Model, message: Message) -> Command<Message> {
        match message {
            // An external request.
            Message::Journal(journal) => match journal {
                JournalMessage::Transition(transition) => {
                    Effect::perform(&cx.io.clock, ReadSystemTime).producing(
                        |EffectOutcome::Succeeded(time)| Message::Time { time, transition },
                    )
                }

                JournalMessage::Recent { count, reply } => {
                    File::read(&cx.io.file).finish_with(move |outcome| {
                        Command::reply(
                            reply,
                            match outcome {
                                EffectOutcome::Succeeded(contents) => Ok(tail(&contents, count)),
                                EffectOutcome::Failed(_) => Err(Error::CouldNotReadJournal),
                            },
                        )
                    })
                }
            },

            // Yeah, I got time.
            Message::Time {
                time,
                transition: Transition { name, from, to },
            } => File::append_line(
                &cx.io.file,
                format!(
                    "[{}]{name}: {from} -> {to}",
                    time.duration_since(UNIX_EPOCH).unwrap().as_millis()
                ),
            )
            .discard_outcome(),
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

#[derive(Debug)]
pub struct Transition {
    name: String,
    from: String,
    to: String,
}

/// The journal's public vocabulary, accepted by the Assembly's Connector.
#[derive(Debug)]
pub enum JournalMessage {
    /// Records one confirmed transition.
    Transition(Transition),

    /// Reads up to `count` trailing lines from the returned file contents.
    ///
    /// Concurrent appends may or may not be reflected; an unterminated last
    /// line is included. Read errors become Journal errors.
    Recent {
        count: usize,
        reply: ReplyWith<Result<Vec<String>>>,
    },
}

impl Assembly for Journal {
    type Config = ();
    type Exports = Connector<JournalMessage>;
    type Io = JournalIo;

    fn install(
        program: &mut ProgramBuilder,
        _: Self::Config,
    ) -> (Connector<JournalMessage>, JournalIo) {
        let recorder = program.component::<Recorder>(());
        (recorder.accepting(Message::Journal), recorder.io.clone())
    }
}

#[cfg(test)]
mod tests;
