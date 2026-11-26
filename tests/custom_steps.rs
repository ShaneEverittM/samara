//! A library gives its own effect an entry point and steps, and its users
//! finish it like any other (ADR-0035). Nothing here is first-party: the
//! `store` module uses only Samara's public API.

use samara::prelude::*;

/// A small key-value store library.
mod store {
    use samara::prelude::*;

    /// Reads one key. The raw result is the bytes, if the key exists.
    #[derive(Debug, PartialEq)]
    pub struct ReadKey(pub String);

    #[derive(Debug, PartialEq)]
    pub struct StoreDown;

    impl EffectDescriptor for ReadKey {
        type Output = Option<Vec<u8>>;
        type Error = StoreDown;
    }

    /// What a refined read can fail with.
    #[derive(Clone, Debug, PartialEq)]
    pub enum ReadError {
        StoreDown,
        Absent,
        NotText,
        NotANumber(String),
    }

    impl From<StoreDown> for ReadError {
        fn from(StoreDown: StoreDown) -> Self {
            Self::StoreDown
        }
    }

    /// The capability a Component names in its `Io`.
    pub type Store = Effect<ReadKey>;

    /// The entry point: `Key::read(&cx.io.store, "name")`.
    pub struct Key;

    impl Key {
        pub fn read(store: &Store, key: impl Into<String>) -> EffectBuilder<ReadKey> {
            Effect::perform(store, ReadKey(key.into()))
        }
    }

    /// Steps on a read whose value is still raw bytes that may be absent.
    pub trait ReadKeySteps {
        /// An absent key becomes a failure.
        fn required(self) -> EffectBuilder<ReadKey, Vec<u8>, ReadError>;
    }

    impl ReadKeySteps for EffectBuilder<ReadKey> {
        fn required(self) -> EffectBuilder<ReadKey, Vec<u8>, ReadError> {
            self.map_outcome(|outcome| {
                match outcome {
                    EffectOutcome::Succeeded(Some(bytes)) => EffectOutcome::Succeeded(bytes),
                    EffectOutcome::Succeeded(None) => EffectOutcome::Failed(ReadError::Absent),
                    EffectOutcome::Failed(error) => EffectOutcome::Failed(error.into()),
                }
            })
        }
    }

    /// Steps on a read whose value is present bytes.
    pub trait BytesSteps {
        /// Decodes the bytes as UTF-8.
        fn text(self) -> EffectBuilder<ReadKey, String, ReadError>;
    }

    impl BytesSteps for EffectBuilder<ReadKey, Vec<u8>, ReadError> {
        fn text(self) -> EffectBuilder<ReadKey, String, ReadError> {
            self.map_outcome(|outcome| {
                match outcome {
                    EffectOutcome::Succeeded(bytes) => {
                        match String::from_utf8(bytes) {
                            Ok(text) => EffectOutcome::Succeeded(text),
                            Err(_) => EffectOutcome::Failed(ReadError::NotText),
                        }
                    }
                    EffectOutcome::Failed(error) => EffectOutcome::Failed(error),
                }
            })
        }
    }

    /// Steps on a read whose value is text.
    pub trait TextSteps {
        /// Parses the text as a number.
        fn number(self) -> EffectBuilder<ReadKey, u64, ReadError>;
    }

    impl TextSteps for EffectBuilder<ReadKey, String, ReadError> {
        fn number(self) -> EffectBuilder<ReadKey, u64, ReadError> {
            self.map_outcome(|outcome| {
                match outcome {
                    EffectOutcome::Succeeded(text) => {
                        match text.trim().parse() {
                            Ok(number) => EffectOutcome::Succeeded(number),
                            Err(_) => EffectOutcome::Failed(ReadError::NotANumber(text)),
                        }
                    }
                    EffectOutcome::Failed(error) => EffectOutcome::Failed(error),
                }
            })
        }
    }
}

use store::{BytesSteps, Key, ReadError, ReadKey, ReadKeySteps, Store, StoreDown, TextSteps};

/// An application Component that uses the library.
struct Settings;

#[derive(Debug)]
enum SettingsMessage {
    /// Load the limit and keep it here.
    LoadLimit,
    Limit(u64),
    NoLimit(ReadError),
    /// Answer a caller with the greeting, without it coming back here.
    Greeting(ReplyWith<Result<String, ReadError>>),
}

#[derive(Debug, Default, PartialEq)]
struct SettingsModel {
    /// How many Messages `update` has handled.
    handled: u64,
    limit: Option<Result<u64, ReadError>>,
}

impl Component for Settings {
    type Io = Store;
    type Connections = ();
    type Configuration = ();
    type Model = SettingsModel;
    type Message = SettingsMessage;

    fn init(_: &Context<Self>) -> Init<SettingsModel, SettingsMessage> {
        Init::default()
    }

    fn update(
        cx: &Context<Self>,
        model: &mut SettingsModel,
        message: SettingsMessage,
    ) -> Command<SettingsMessage> {
        model.handled += 1;
        match message {
            SettingsMessage::LoadLimit => {
                Key::read(&cx.io, "limit")
                    .required()
                    .text()
                    .number()
                    .on_success(SettingsMessage::Limit)
                    .on_failure(SettingsMessage::NoLimit)
            }
            SettingsMessage::Limit(limit) => {
                model.limit = Some(Ok(limit));
                Command::none()
            }
            SettingsMessage::NoLimit(error) => {
                model.limit = Some(Err(error));
                Command::none()
            }
            SettingsMessage::Greeting(reply) => {
                Key::read(&cx.io, "greeting")
                    .required()
                    .text()
                    .divert_to(reply)
                    .on_success(Ok)
                    .on_failure(Err)
            }
        }
    }
}

/// Asks Settings for the greeting and records the answer.
struct Visitor;

enum VisitorMessage {
    Ask,
    Heard(RequestOutcome<Result<String, ReadError>>),
}

impl Component for Visitor {
    type Io = ();
    type Connections = ComponentRef<Settings>;
    type Configuration = ();
    type Model = Vec<RequestOutcome<Result<String, ReadError>>>;
    type Message = VisitorMessage;

    fn init(_: &Context<Self>) -> Init<Self::Model, VisitorMessage> {
        Init::default()
    }

    fn update(
        cx: &Context<Self>,
        heard: &mut Self::Model,
        message: VisitorMessage,
    ) -> Command<VisitorMessage> {
        match message {
            VisitorMessage::Ask => {
                Command::request(&cx.to, SettingsMessage::Greeting)
                    .on_outcome(VisitorMessage::Heard)
            }
            VisitorMessage::Heard(outcome) => {
                heard.push(outcome);
                Command::none()
            }
        }
    }
}

struct Fixture {
    runtime: ControlledRuntime,
    settings: ComponentRef<Settings>,
    visitor: ComponentRef<Visitor>,
}

fn fixture() -> Fixture {
    let mut builder = Program::builder();
    let settings: ComponentRef<Settings> = builder.add_component::<Settings>(()).into();
    let visitor = builder.add_component::<Visitor>(settings.clone());
    let runtime = ControlledRuntime::builder(builder.build().unwrap())
        .build()
        .unwrap();
    Fixture {
        runtime,
        settings,
        visitor: visitor.into(),
    }
}

/// Completes the one pending read with the store's raw result. A controlled
/// test speaks the descriptor's own types; the steps run afterwards.
fn complete_read(
    runtime: &mut ControlledRuntime,
    key: &str,
    raw: EffectOutcome<Option<Vec<u8>>, StoreDown>,
) {
    runtime.run_until_idle().unwrap();
    let read = runtime.next_effect::<ReadKey>().unwrap();
    assert_eq!(read.intent, ReadKey(key.into()));
    runtime.complete(read, raw).unwrap();
    runtime.run_until_idle().unwrap();
}

fn load_limit(raw: EffectOutcome<Option<Vec<u8>>, StoreDown>) -> Option<Result<u64, ReadError>> {
    let Fixture {
        mut runtime,
        settings,
        ..
    } = fixture();
    runtime.send(&settings, SettingsMessage::LoadLimit).unwrap();
    complete_read(&mut runtime, "limit", raw);
    let SettingsModel { handled, limit } = runtime.state(&settings).unwrap();
    // The request, and its result as one Message.
    assert_eq!(*handled, 2);
    let limit = limit.clone();
    assert!(runtime.cancel().unwrap().is_clean());
    limit
}

#[test]
fn each_step_refines_the_result_or_fails_it() {
    assert_eq!(
        load_limit(EffectOutcome::Succeeded(Some(b" 40 ".to_vec()))),
        Some(Ok(40))
    );
    assert_eq!(
        load_limit(EffectOutcome::Failed(StoreDown)),
        Some(Err(ReadError::StoreDown))
    );
    assert_eq!(
        load_limit(EffectOutcome::Succeeded(None)),
        Some(Err(ReadError::Absent))
    );
    assert_eq!(
        load_limit(EffectOutcome::Succeeded(Some(vec![0xff]))),
        Some(Err(ReadError::NotText))
    );
    assert_eq!(
        load_limit(EffectOutcome::Succeeded(Some(b"many".to_vec()))),
        Some(Err(ReadError::NotANumber("many".into())))
    );
}

#[test]
fn custom_steps_compose_with_a_diverted_reply() {
    let Fixture {
        mut runtime,
        settings,
        visitor,
    } = fixture();

    runtime.send(&visitor, VisitorMessage::Ask).unwrap();
    complete_read(
        &mut runtime,
        "greeting",
        EffectOutcome::Succeeded(Some(b"hello".to_vec())),
    );
    runtime.send(&visitor, VisitorMessage::Ask).unwrap();
    complete_read(&mut runtime, "greeting", EffectOutcome::Succeeded(None));

    assert_eq!(
        runtime.state(&visitor).unwrap(),
        &[
            RequestOutcome::Replied(Ok("hello".into())),
            RequestOutcome::Replied(Err(ReadError::Absent)),
        ]
    );
    // Neither result came back through `update`.
    assert_eq!(runtime.state(&settings).unwrap().handled, 2);
    assert!(runtime.cancel().unwrap().is_clean());
}
