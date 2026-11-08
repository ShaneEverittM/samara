use std::convert::Infallible;
use std::time::SystemTime;

use samara::prelude::*;

const JOURNAL: &str = "pulse/journal";

protocol! {
    pub type Journal => enum ProtocolMessage {
        Transition { name: String, from: String, to: String },
        Recent(usize) -> Vec<String>,
    }
}

/// Appends one line to the journal file.
pub struct AppendLine(pub String);

impl EffectDescriptor for AppendLine {
    type Output = ();
    type Error = std::io::Error;
}

/// Asks the host what time it is. `update` never reads a clock itself.
pub struct Now;

impl EffectDescriptor for Now {
    type Output = SystemTime;
    type Error = Infallible;
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

struct FsJournal {
    append: EffectCapability<AppendLine>,
    clock: EffectCapability<Now>,
}

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
                    let _ = (&self.append, &self.clock, name, from, to);
                    todo!()
                }
                (Model::Idle, ProtocolMessage::Recent(_)) => todo!(),
            },
        }
    }
}

/// The drivers a host must bind for the journal, one field per capability.
///
/// `Drivers::default()` gives the file appender and system clock on the live
/// builder (feature `live-drivers`) and [`Controlled`] markers on the controlled
/// builder. Fill the fields by hand to substitute either.
pub struct Drivers<R: RuntimeBuilder> {
    pub append: R::EffectBinding<AppendLine>,
    pub clock: R::EffectBinding<Now>,
}

impl<R: RuntimeBuilder> Bindings<R> for Drivers<R> {
    fn bind(self, runtime: R) -> R {
        runtime
            .bind_effect::<AppendLine>(self.append)
            .bind_effect::<Now>(self.clock)
    }
}

impl Default for Drivers<ControlledRuntimeBuilder> {
    fn default() -> Self {
        Self {
            append: Controlled,
            clock: Controlled,
        }
    }
}

#[cfg(feature = "live-drivers")]
impl Default for Drivers<LiveRuntimeBuilder> {
    fn default() -> Self {
        Self {
            append: Box::new(FsAppend::new(FsAppend::DEFAULT_PATH)),
            clock: Box::new(SystemClock),
        }
    }
}

#[cfg(feature = "live-drivers")]
pub use live::{FsAppend, SystemClock};

/// Live implementations of the journal's effects.
#[cfg(feature = "live-drivers")]
mod live {
    use std::convert::Infallible;
    use std::path::PathBuf;
    use std::time::SystemTime;

    use samara::prelude::*;

    use super::{AppendLine, Now};

    /// Appends lines to a file on disk.
    pub struct FsAppend {
        path: PathBuf,
    }

    impl FsAppend {
        /// Where [`super::Drivers::default`] writes.
        pub const DEFAULT_PATH: &str = "pulse.journal";

        pub fn new(path: impl Into<PathBuf>) -> Self {
            Self { path: path.into() }
        }
    }

    impl EffectDriver<AppendLine> for FsAppend {
        fn execute(&self, AppendLine(line): AppendLine) -> BoxFuture<Result<(), std::io::Error>> {
            let path = self.path.clone();
            Box::pin(async move {
                use tokio::io::AsyncWriteExt;
                let mut file = tokio::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .await?;
                file.write_all(format!("{line}\n").as_bytes()).await
            })
        }
    }

    /// Reads the system clock.
    pub struct SystemClock;

    impl EffectDriver<Now> for SystemClock {
        fn execute(&self, _: Now) -> BoxFuture<Result<SystemTime, Infallible>> {
            Box::pin(async { Ok(SystemTime::now()) })
        }
    }
}

// The protocol type is the Subprogram, so `FsJournal`, its Model, and its
// Message stay private. Implementing it on the Component would make all three
// public, because `Component`'s associated types are part of its interface.
impl Subprogram for Journal {
    type Config = ();
    type Exports = Port<Journal>;
    type Drivers<R: RuntimeBuilder> = Drivers<R>;

    fn install(program: &mut ProgramBuilder, _: Self::Config) -> Port<Journal> {
        let append = program.effect::<AppendLine>();
        let clock = program.effect::<Now>();
        let journal = program.port::<Journal>(PortId::new(JOURNAL));
        let fs = program.component(ComponentId::new("fs-journal"), FsJournal { append, clock });
        program.bind_port(&journal, &fs);

        journal
    }
}
