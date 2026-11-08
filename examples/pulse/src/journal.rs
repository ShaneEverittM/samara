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
pub struct AppendLineToFile(pub String);

impl EffectDescriptor for AppendLineToFile {
    type Output = ();
    type Error = std::io::Error;
}

/// Asks the host what time it is. `update` never reads a clock itself.
pub struct GetTime;

impl EffectDescriptor for GetTime {
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

/// Applies the hold policy and records confirmed transitions.
struct Recorder {
    append: EffectCapability<AppendLineToFile>,
    clock: EffectCapability<GetTime>,
}

impl Component for Recorder {
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
pub struct Drivers<R>
where
    R: RuntimeBuilder,
{
    pub append: R::EffectBinding<AppendLineToFile>,
    pub clock: R::EffectBinding<GetTime>,
}

impl<R> Bindings<R> for Drivers<R>
where
    R: RuntimeBuilder,
{
    fn bind(self, runtime: R) -> R {
        runtime
            .bind_effect::<AppendLineToFile>(self.append)
            .bind_effect::<GetTime>(self.clock)
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
            append: Box::new(AppendLineWithTokio::new(AppendLineWithTokio::DEFAULT_PATH)),
            clock: Box::new(GetTimeWithSystemClock),
        }
    }
}

#[cfg(feature = "live-drivers")]
pub use live::{AppendLineWithTokio, GetTimeWithSystemClock};

/// Live implementations of the journal's effects.
#[cfg(feature = "live-drivers")]
mod live {
    use std::convert::Infallible;
    use std::path::PathBuf;
    use std::time::SystemTime;

    use samara::prelude::*;

    use super::{AppendLineToFile, GetTime};

    /// Appends lines to a file on disk.
    pub struct AppendLineWithTokio {
        path: PathBuf,
    }

    impl AppendLineWithTokio {
        /// Where [`super::Drivers::default`] writes.
        pub const DEFAULT_PATH: &str = "pulse.journal";

        pub fn new(path: impl Into<PathBuf>) -> Self {
            Self { path: path.into() }
        }
    }

    impl EffectDriver<AppendLineToFile> for AppendLineWithTokio {
        fn execute(&self, AppendLineToFile(line): AppendLineToFile) -> BoxFuture<Result<(), std::io::Error>> {
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
    pub struct GetTimeWithSystemClock;

    impl EffectDriver<GetTime> for GetTimeWithSystemClock {
        fn execute(&self, _: GetTime) -> BoxFuture<Result<SystemTime, Infallible>> {
            Box::pin(async { Ok(SystemTime::now()) })
        }
    }
}

// The protocol type is the Subprogram, so `Recorder`, its Model, and its
// Message stay private. Implementing it on the Component would make all three
// public, because `Component`'s associated types are part of its interface.
impl Subprogram for Journal {
    type Config = ();
    type Exports = Port<Journal>;
    type Drivers<R: RuntimeBuilder> = Drivers<R>;

    fn install(program: &mut ProgramBuilder, _: Self::Config) -> Port<Journal> {
        let append = program.effect::<AppendLineToFile>();
        let clock = program.effect::<GetTime>();
        let journal = program.port::<Journal>(PortId::new(JOURNAL));
        let recorder = program.component(ComponentId::new("journal"), Recorder { append, clock });
        program.bind_port(&journal, &recorder);

        journal
    }
}
