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

struct StdinProbe {
    stdin: SourceCapability<StdinLines>,
}

impl Component for StdinProbe {
    type Model = StdinModel;
    type Message = StdinMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::default()
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
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

    fn subscriptions(&self, model: &Self::Model) -> Subscriptions<Self::Message> {
        if model.terminal.is_some() {
            Subscriptions::none()
        } else {
            Subscriptions::one(Subscription::source(
                &self.stdin,
                SubscriptionId::new(STDIN_SUBSCRIPTION),
                StdinLines::new(),
            ))
        }
    }
}

fn stdin_program() -> (
    Program,
    ComponentRef<StdinProbe>,
    SourceCapability<StdinLines>,
) {
    let mut builder = Program::builder();
    let stdin = builder.source::<StdinLines>();
    let probe = builder.component(
        ComponentId::new("stdin-probe"),
        StdinProbe {
            stdin: stdin.clone(),
        },
    );
    (builder.build().expect("valid stdin program"), probe, stdin)
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
    let (program, probe, _stdin) = stdin_program();
    let mut runtime = ControlledRuntime::builder(program)
        .control_source::<StdinLines>()
        .build()
        .expect("valid controlled stdin binding");

    for line in ["first", "", "last without newline"] {
        runtime
            .emit_source::<StdinProbe, StdinLines>(
                &probe,
                &SubscriptionId::new(STDIN_SUBSCRIPTION),
                SourceEvent::Item(line.to_owned()),
            )
            .expect("line accepted");
    }
    runtime
        .emit_source::<StdinProbe, StdinLines>(
            &probe,
            &SubscriptionId::new(STDIN_SUBSCRIPTION),
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
    let (program, probe, _stdin) = stdin_program();
    let mut runtime = ControlledRuntime::builder(program)
        .control_source::<StdinLines>()
        .build()
        .expect("valid controlled stdin binding");
    let failure = StdinError::new(StdinErrorKind::InvalidUtf8, "scripted invalid line");

    runtime
        .emit_source::<StdinProbe, StdinLines>(
            &probe,
            &SubscriptionId::new(STDIN_SUBSCRIPTION),
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
    let (program, _probe, _stdin) = stdin_program();
    assert!(LiveRuntime::builder(program).build().is_err());

    let (program, _probe, stdin) = stdin_program();
    assert!(
        LiveRuntime::builder(program)
            .bind_stdin(&stdin)
            .build()
            .is_ok()
    );

    let (program, _probe, _stdin) = stdin_program();
    let mut foreign = Program::builder();
    let foreign_stdin = foreign.source::<StdinLines>();
    assert!(
        LiveRuntime::builder(program)
            .bind_stdin(&foreign_stdin)
            .build()
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn live_stdin_binding_accepts_a_composed_capability() {
    let mut builder = Program::builder();
    let stdin = builder.source::<Framed<StdinLines, LineLength>>();
    let program = builder.build().expect("valid composed stdin program");

    assert!(
        LiveRuntime::builder(program)
            .bind_stdin(&stdin)
            .build()
            .is_ok()
    );
}

#[cfg(unix)]
#[test]
fn live_stdin_rejects_duplicate_or_competing_process_bindings() {
    let (program, _probe, stdin) = stdin_program();
    assert!(
        LiveRuntime::builder(program)
            .bind_stdin(&stdin)
            .bind_stdin(&stdin)
            .build()
            .is_err()
    );

    let mut builder = Program::builder();
    let first = builder.source::<StdinLines>();
    let _second = builder.source::<StdinLines>();
    let program = builder.build().expect("two declarations are valid");
    assert!(
        LiveRuntime::builder(program)
            .bind_stdin(&first)
            .build()
            .is_err()
    );

    let mut builder = Program::builder();
    let first = builder.source::<StdinLines>();
    let second = builder.source::<StdinLines>();
    let program = builder.build().expect("two declarations are valid");
    assert!(
        LiveRuntime::builder(program)
            .bind_stdin(&first)
            .bind_stdin(&second)
            .build()
            .is_err()
    );
}
