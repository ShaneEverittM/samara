use samara::prelude::*;

const STDIN_SUBSCRIPTION: &str = "stdin";

#[derive(Clone, Debug, PartialEq, Eq)]
struct LineLength;

impl Decoder for LineLength {
    type Chunk = String;
    type Frame = usize;
    type Error = std::convert::Infallible;
    type State = ();

    fn start(&self) -> Self::State {}

    fn push(
        &self,
        _state: &mut Self::State,
        line: Self::Chunk,
    ) -> Result<Vec<Self::Frame>, Self::Error> {
        Ok(vec![line.len()])
    }

    fn finish(&self, _state: &mut Self::State) -> Result<Vec<Self::Frame>, Self::Error> {
        Ok(Vec::new())
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct StdinModel {
    lines: Vec<String>,
    terminal: Option<StdinTerminal>,
}

#[derive(Debug, PartialEq, Eq)]
enum StdinTerminal {
    Ended,
    Failed(StdinError),
}

enum StdinMessage {
    Input(SourceEvent<String, StdinError>),
}

impl From<SourceEvent<String, StdinError>> for StdinMessage {
    fn from(event: SourceEvent<String, StdinError>) -> Self {
        Self::Input(event)
    }
}

struct StdinProbe;

impl Component for StdinProbe {
    type Io = Source<StdinLines>;
    type Config = ();
    type Model = StdinModel;
    type Message = StdinMessage;

    fn init(_config: &Self::Config, _: &Self::Io) -> Init<Self::Model, Self::Message> {
        Init::default()
    }

    fn update(
        _config: &Self::Config,
        _: &Self::Io,
        model: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        match message {
            StdinMessage::Input(SourceEvent::Item(line)) => model.lines.push(line),
            StdinMessage::Input(SourceEvent::Failed(error)) => {
                model.terminal = Some(StdinTerminal::Failed(error));
            }
            StdinMessage::Input(SourceEvent::Ended) => {
                model.terminal = Some(StdinTerminal::Ended);
            }
        }
        Command::none()
    }

    fn subscriptions(
        _: &Self::Config,
        stdin: &Self::Io,
        model: &Self::Model,
    ) -> Subscriptions<Self::Message> {
        if model.terminal.is_some() {
            Subscriptions::none()
        } else {
            Subscriptions::one(
                Subscription::source(stdin, StdinLines::new()).key(STDIN_SUBSCRIPTION),
            )
        }
    }
}

fn stdin_program() -> (Program, InstalledComponent<StdinProbe>) {
    let mut builder = Program::builder();
    let probe = builder
        .component::<StdinProbe>(())
        .name("stdin-probe")
        .finish()
        .expect("component registration");
    (builder.build().expect("valid stdin program"), probe)
}

/// Owns the capabilities `C` and never uses them; only their declaration
/// matters to the live binding rules below.
struct Owns<C>(std::marker::PhantomData<fn() -> C>);

impl<C: Capabilities> Component for Owns<C> {
    type Io = C;
    type Config = ();
    type Model = ();
    type Message = ();

    fn init(_: &(), _: &Self::Io) -> Init<(), ()> {
        Init::default()
    }

    fn update(_: &(), _: &Self::Io, _: &mut (), _: ()) -> Command<()> {
        Command::none()
    }
}

/// A Program of one Component that owns `C`, and those capabilities.
fn program_owning<C: Capabilities>() -> (Program, C) {
    let mut builder = Program::builder();
    let owner = builder
        .component::<Owns<C>>(())
        .finish()
        .expect("component registration");
    let io = owner.io.clone();
    (builder.build().expect("valid program"), io)
}

#[test]
fn stdin_descriptor_and_errors_are_typed_fixture_data() {
    fn assert_source<S>()
    where
        S: SourceDescriptor<Item = String, Error = StdinError>,
    {
    }

    assert_source::<StdinLines>();
    assert_eq!(StdinLines::new(), StdinLines);

    let read = StdinError::new(StdinErrorKind::Read, "controlled read failure");
    assert_eq!(read.kind(), StdinErrorKind::Read);
    assert_eq!(read.message(), "controlled read failure");
    assert!(read.to_string().contains("controlled read failure"));

    let utf8 = StdinError::new(StdinErrorKind::InvalidUtf8, "invalid byte 0xff");
    assert_eq!(utf8.kind(), StdinErrorKind::InvalidUtf8);
    assert_eq!(utf8.message(), "invalid byte 0xff");
}

#[test]
fn controlled_stdin_delivers_lines_and_normal_eof() {
    let (program, probe) = stdin_program();
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
        .build()
        .expect("valid controlled stdin binding");

    for line in ["first", "", "last without newline"] {
        runtime
            .emit_source::<StdinProbe, StdinLines>(
                &probe,
                &SubscriptionId::key(STDIN_SUBSCRIPTION),
                SourceEvent::Item(line.to_owned()),
            )
            .expect("line accepted");
    }
    runtime
        .emit_source::<StdinProbe, StdinLines>(
            &probe,
            &SubscriptionId::key(STDIN_SUBSCRIPTION),
            SourceEvent::Ended,
        )
        .expect("EOF accepted");
    runtime.run_until_idle().expect("stdin events delivered");

    assert_eq!(
        runtime.state(&probe).expect("stdin model"),
        &StdinModel {
            lines: vec![
                "first".to_owned(),
                String::new(),
                "last without newline".to_owned(),
            ],
            terminal: Some(StdinTerminal::Ended),
        }
    );
    assert!(runtime.cancel().expect("clean close").is_clean());
}

#[test]
fn controlled_stdin_delivers_typed_failure_without_live_fallback() {
    let (program, probe) = stdin_program();
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
        .build()
        .expect("valid controlled stdin binding");
    let failure = StdinError::new(StdinErrorKind::InvalidUtf8, "scripted invalid line");

    runtime
        .emit_source::<StdinProbe, StdinLines>(
            &probe,
            &SubscriptionId::key(STDIN_SUBSCRIPTION),
            SourceEvent::Failed(failure.clone()),
        )
        .expect("failure accepted");
    runtime.run_until_idle().expect("failure delivered");

    assert_eq!(
        runtime.state(&probe).expect("stdin model").terminal,
        Some(StdinTerminal::Failed(failure))
    );
    assert!(runtime.cancel().expect("clean close").is_clean());
}

#[cfg(unix)]
#[test]
fn live_stdin_binding_is_exact_and_required() {
    let (program, probe) = stdin_program();
    assert!(
        Runtime::builder(program)
            .supply(&probe.io, samara::StdinDriver)
            .build()
            .is_ok()
    );

    let (program, _probe) = stdin_program();
    let error = Runtime::builder(program)
        .build()
        .err()
        .expect("a stdin capability without a Driver");
    let error = error.to_string();
    assert!(error.contains("stdin-probe"), "{error}");
    assert!(error.contains("0 live bindings"), "{error}");
}

#[cfg(unix)]
#[test]
fn live_stdin_binding_accepts_a_composed_capability() {
    let (program, (stdin, tcp)) =
        program_owning::<(Source<Framed<StdinLines, LineLength>>, Source<TcpBytes>)>();

    assert!(
        Runtime::builder(program)
            .supply(&stdin, samara::StdinDriver)
            .supply(&tcp, samara::DrivenSource::new(samara::TcpDriver))
            .build()
            .is_ok()
    );
}

#[cfg(unix)]
#[test]
fn live_stdin_rejects_duplicate_or_competing_process_bindings() {
    // Process stdin is one resource: two bindings conflict even though each
    // is for a different capability.
    let (program, caps) = program_owning::<(Source<StdinLines>, Source<StdinLines>)>();
    let error = Runtime::builder(program)
        .supply(&caps, (samara::StdinDriver, samara::StdinDriver))
        .build()
        .err()
        .expect("two stdin declarations are ambiguous even without subscriptions");
    assert!(error.to_string().contains("duplicate or ambiguous"));

    let (program, caps) =
        program_owning::<(Source<StdinLines>, Source<Framed<StdinLines, LineLength>>)>();
    assert!(
        Runtime::builder(program)
            .supply(&caps, (samara::StdinDriver, samara::StdinDriver))
            .build()
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn live_stdin_rejects_a_conflicting_type_wide_driver_in_either_order() {
    struct UnusedStdinDriver;
    impl SourceDriver<StdinLines> for UnusedStdinDriver {
        fn run(&self, _: StdinLines, _: SourceSink<StdinLines>) -> BoxFuture<()> {
            panic!("build must not run a source driver");
        }
    }

    for stdin_first in [false, true] {
        let (program, caps) = program_owning::<(Source<StdinLines>, Source<StdinLines>)>();
        let builder = Runtime::builder(program);
        let builder = if stdin_first {
            builder.supply(&caps, (StdinDriver, DrivenSource::new(UnusedStdinDriver)))
        } else {
            builder.supply(&caps, (DrivenSource::new(UnusedStdinDriver), StdinDriver))
        };
        assert!(builder.build().is_err());
    }
}
