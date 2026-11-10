use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Utc};
use samara::prelude::*;
use serde::Deserialize;

const CLI_SUBSCRIPTION: &str = "commands";
const TIME_SERVER_PORT: &str = "TimeServer";

enum HttpTimeMessage {
    // A tick to drive our model.
    Tick,

    // Received when we are asked for our current time; the token routes the reply.
    GetTime(ReplyWith<Option<DateTime<Utc>>>),

    // One terminal Message keeps poll cleanup common to every outcome.
    PollFinished(Result<DateTime<Utc>, GetTimeError>),
}

impl From<EffectOutcome<TimeResponse, HttpResponseError>> for HttpTimeMessage {
    fn from(outcome: EffectOutcome<TimeResponse, HttpResponseError>) -> Self {
        use EffectOutcome::*;

        Self::PollFinished(match outcome {
            Succeeded(response) => Ok(response.data.iso),
            Failed(EffectFailure::Error(error)) => Err(GetTimeError::Response(error)),
            Failed(EffectFailure::Cancelled(reason)) => Err(GetTimeError::Cancelled(reason)),
        })
    }
}

impl From<TimeServerMessage> for HttpTimeMessage {
    fn from(value: TimeServerMessage) -> Self {
        match value {
            TimeServerMessage::GetCurrentTime(reply_with) => Self::GetTime(reply_with),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum HttpTimeModel {
    Idle { current_time: Option<DateTime<Utc>> },
    Polling { current_time: Option<DateTime<Utc>> },
}

impl HttpTimeModel {
    fn current_time(&self) -> Option<DateTime<Utc>> {
        let (Self::Idle { current_time } | Self::Polling { current_time }) = self;
        *current_time
    }
}

/// HTTP-backed implementation of the provider-neutral time vocabulary.
struct HttpTimeServer;

struct HttpTimeServerConfig {}

impl HttpTimeServer {
    const TICK: Command<HttpTimeMessage> =
        Command::after(Duration::from_secs(1), HttpTimeMessage::Tick);
}

impl Component for HttpTimeServer {
    type Requires = (Effect<HttpRequest>, Effect<PrintStderr>);
    type Config = HttpTimeServerConfig;
    type Model = HttpTimeModel;
    type Message = HttpTimeMessage;

    fn init(
        _config: &Self::Config,
        _capabilities: &Capabilities<Self::Requires>,
    ) -> Init<Self::Model, Self::Message> {
        Init::new(HttpTimeModel::Idle { current_time: None }).with_command(Command::after(
            Duration::from_secs(1),
            HttpTimeMessage::Tick,
        ))
    }

    fn update(
        _config: &Self::Config,
        capabilities: &Capabilities<Self::Requires>,
        model: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        use HttpTimeMessage::{GetTime, PollFinished, Tick};
        use HttpTimeModel::{Idle, Polling};

        match (model, message) {
            (state @ Idle { .. }, Tick) => {
                *state = Polling {
                    current_time: state.current_time(),
                };

                let get = HttpRequest::get("https://api.coinbase.com/v2/time")
                    .on_response()
                    .require_success()
                    .json::<TimeResponse>()
                    .into_command(&capabilities.0);

                Command::batch([get, Self::TICK])
            }

            // Keep ticking while busy, without issuing another HTTP poll.
            (Polling { .. }, Tick) => Self::TICK,

            (state @ Polling { .. }, PollFinished(result)) => {
                let (current_time, command) = match result {
                    Ok(time) => (Some(time), Command::none()),
                    Err(error) => (
                        state.current_time(),
                        samara::eprintln!(&capabilities.1, "Failed to get time: {error}"),
                    ),
                };
                *state = Idle { current_time };
                command
            }

            // There is no active poll for this completion to settle.
            (Idle { .. }, PollFinished(_)) => Command::none(),

            // Cached reads are available in either state.
            (state, GetTime(reply_with)) => Command::reply(reply_with, state.current_time()),
        }
    }
}

/// Commands understood by the interactive terminal Component.
#[derive(Debug, PartialEq, Eq)]
enum CliCommand {
    /// Print the supplied text through Samara's standard-output effect.
    Print(String),

    /// Ask the time provider for its most recently observed value.
    GetTime,
}

/// Every input that can drive the CLI's state transition.
enum CliMessage {
    /// One valid command parsed from the ongoing input stream.
    Command(CliCommand),

    /// A line did not name one of the CLI's supported commands.
    InvalidCommand(String),

    /// Process standard input reached EOF normally.
    InputEnded,

    /// Process standard input failed before reaching EOF.
    InputFailed(StdinError),

    /// Terminal outcome of a request sent to the time-server Port.
    TimeReceived(RequestOutcome<Option<DateTime<Utc>>>),
}

/// Parses one terminal line without performing I/O or mutating Component state.
fn parse_cli_command(line: String) -> CliMessage {
    let line = line.trim();
    let mut parts = line.splitn(2, char::is_whitespace);
    let command = parts.next().unwrap_or_default();
    let argument = parts.next().unwrap_or_default().trim();

    match (command, argument) {
        ("print", text) if !text.is_empty() => {
            CliMessage::Command(CliCommand::Print(text.to_owned()))
        }
        ("time", "") => CliMessage::Command(CliCommand::GetTime),
        _ => CliMessage::InvalidCommand(line.to_owned()),
    }
}

/// Mutable behavioral state owned by the command-line Component.
#[derive(Default)]
struct CliModel {
    /// Withdraws terminal input after its normal or failed terminal event.
    input_closed: bool,
}

/// Interactive Component that translates terminal commands into Samara intent.
struct Cli;

struct CliConfig {
    time_server: Port<TimeServerMessage>,
}

impl Component for Cli {
    type Requires = (Source<StdinLines>, Effect<PrintStdout>, Effect<PrintStderr>);
    type Config = CliConfig;
    type Model = CliModel;
    type Message = CliMessage;

    fn init(
        _config: &Self::Config,
        capabilities: &Capabilities<Self::Requires>,
    ) -> Init<Self::Model, Self::Message> {
        Init::default().with_command(samara::println!(
            &capabilities.1,
            "Commands: `print <text>` or `time`"
        ))
    }

    fn update(
        config: &Self::Config,
        capabilities: &Capabilities<Self::Requires>,
        model: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        match message {
            CliMessage::Command(CliCommand::Print(text)) => {
                samara::println!(&capabilities.1, "{text}")
            }
            CliMessage::Command(CliCommand::GetTime) => Command::request_to(
                &config.time_server,
                TimeServerMessage::GetCurrentTime,
                CliMessage::TimeReceived,
            ),
            CliMessage::InvalidCommand(command) => samara::eprintln!(
                &capabilities.2,
                "Unknown command `{command}`; use `print <text>` or `time`"
            ),
            CliMessage::InputEnded => {
                model.input_closed = true;
                Command::none()
            }
            CliMessage::InputFailed(error) => {
                model.input_closed = true;
                samara::eprintln!(&capabilities.2, "Failed to read standard input: {error}")
            }
            CliMessage::TimeReceived(RequestOutcome::Replied(Some(time))) => {
                samara::println!(&capabilities.1, "Current time: {time}")
            }
            CliMessage::TimeReceived(RequestOutcome::Replied(None)) => {
                samara::println!(
                    &capabilities.1,
                    "The time server has not received a value yet"
                )
            }
            CliMessage::TimeReceived(RequestOutcome::Failed(error)) => {
                samara::eprintln!(&capabilities.2, "Time request failed: {error:?}")
            }
            CliMessage::TimeReceived(RequestOutcome::TimedOut) => {
                samara::eprintln!(&capabilities.2, "Time request timed out")
            }
            CliMessage::TimeReceived(RequestOutcome::Cancelled) => {
                samara::eprintln!(&capabilities.2, "Time request was cancelled")
            }
        }
    }

    fn subscriptions(
        _config: &Self::Config,
        capabilities: &Capabilities<Self::Requires>,
        model: &Self::Model,
    ) -> Subscriptions<Self::Message> {
        if model.input_closed {
            return Subscriptions::none();
        }

        Subscriptions::one(Subscription::source_with(
            &capabilities.0,
            SubscriptionId::new(CLI_SUBSCRIPTION),
            StdinLines::new(),
            |event| match event {
                SourceEvent::Item(line) => parse_cli_command(line),
                SourceEvent::Ended => CliMessage::InputEnded,
                SourceEvent::Failed(error) => CliMessage::InputFailed(error),
            },
        ))
    }
}

#[derive(Debug, thiserror::Error)]
enum GetTimeError {
    #[error(transparent)]
    Response(#[from] HttpResponseError),

    #[error("request was cancelled: {0:?}")]
    Cancelled(CancelReason),
}

#[derive(Deserialize)]
struct TimeResponseData {
    iso: DateTime<Utc>,
}

#[derive(Deserialize)]
struct TimeResponse {
    data: TimeResponseData,
}

/// Provider-neutral vocabulary of the time service, carried by
/// `Port<TimeServerMessage>`.
#[derive(Debug)]
enum TimeServerMessage {
    /// Asks for the provider's most recently observed time; `None` until it
    /// has one.
    GetCurrentTime(ReplyWith<Option<DateTime<Utc>>>),
}

#[cfg(unix)]
#[tokio::main]
async fn main() -> Result<()> {
    // Start building a Samara program, which is a declaration of the topology.
    let (mut builder, (http, stdout, stderr, cli_input)) = Program::builder::<(
        Effect<HttpRequest>,
        Effect<PrintStdout>,
        Effect<PrintStderr>,
        Source<StdinLines>,
    )>();

    // Declare every world boundary before constructing Components. These inert
    // capabilities are both the dependency declarations and the only way the
    // Component can issue the corresponding Effects.

    // A Port names the provider-neutral time service consumed by the CLI.
    let time_port = builder.port::<TimeServerMessage>(PortId::new(TIME_SERVER_PORT));

    // The concrete provider happens to obtain time over HTTP.
    let http_time_server = builder
        .component::<HttpTimeServer>(
            ComponentId::new("http-time-server"),
            HttpTimeServerConfig {},
        )
        .with_capabilities((http, stderr.clone()))
        .finish()
        .expect("component registration");

    // Program assembly chooses which concrete Component provides that service.
    builder.connect(&time_port, &http_time_server.connector());

    // Raw terminal lines cross the runtime boundary as an ongoing Source. The
    // CLI owns parsing, presentation, and its dependency on the time service.
    builder
        .component::<Cli>(
            ComponentId::new("cli"),
            CliConfig {
                time_server: time_port,
            },
        )
        .with_capabilities((cli_input.clone(), stdout, stderr))
        .finish()
        .expect("component registration");

    // Build the program, this catches errors like unbound ports, etc.
    let program = builder.build()?;

    // Configure a runtime for real usage...
    let runtime = Runtime::builder(program)
        .with_drivers((
            samara::HttpDriver::new(),
            samara::StdoutDriver::new(),
            samara::StderrDriver::new(),
            samara::StdinDriver,
        ))
        .build()?;

    // We've now set up the program, runtime, and ingress points, so start the runtime.
    let mut runtime_task = runtime.spawn();

    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    // The host supervises runtime failure and its own Ctrl-C shutdown policy.
    // Stdin EOF remains an application event for the CLI; it does not
    // implicitly terminate the other Component's ongoing timer work.
    tokio::select! {
        result = runtime_task.run_forever() => {
            result?;
            return Ok(());
        }
        signal = &mut ctrl_c => signal?,
    }

    // Cancellation is explicit because the HTTP time server deliberately has
    // ongoing timer work. The stdin binding interrupts and joins its reader as
    // part of the same structured shutdown.
    runtime_task.shutdown(Shutdown::Cancel).await?;

    Ok(())
}

#[cfg(not(unix))]
fn main() -> Result<()> {
    anyhow::bail!("the first-party live StdinLines binding currently requires Unix")
}

#[cfg(test)]
mod polling_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn cli_fixture() -> (CliConfig, Capabilities<<Cli as Component>::Requires>) {
        let (mut builder, (declared_0, declared_1, declared_2)) =
            Program::builder::<(Source<StdinLines>, Effect<PrintStdout>, Effect<PrintStderr>)>();
        (
            CliConfig {
                time_server: builder.port(PortId::new(TIME_SERVER_PORT)),
            },
            (declared_0, declared_1, declared_2),
        )
    }

    #[test]
    fn cli_commands_declare_print_and_time_request_intent() {
        let (cli, cli_capabilities) = cli_fixture();
        let mut model = Cli::init(&cli, &cli_capabilities).model;

        let print = Cli::update(
            &cli,
            &cli_capabilities,
            &mut model,
            CliMessage::Command(CliCommand::Print("hello Samara".to_owned())),
        );
        assert_eq!(
            print
                .effect_intent::<PrintStdout>()
                .expect("print should request stdout")
                .as_str(),
            "hello Samara\n"
        );

        let time = Cli::update(
            &cli,
            &cli_capabilities,
            &mut model,
            CliMessage::Command(CliCommand::GetTime),
        );
        let request = time
            .into_request_to::<TimeServerMessage>()
            .unwrap_or_else(|_| panic!("`time` should request the time server"));
        assert!(matches!(
            request.message,
            TimeServerMessage::GetCurrentTime(_)
        ));
        assert!(matches!(
            request.reply(RequestOutcome::Replied(None::<DateTime<Utc>>)),
            CliMessage::TimeReceived(RequestOutcome::Replied(None))
        ));
    }

    #[test]
    fn cli_subscription_parses_input_and_withdraws_after_eof() {
        let (cli, cli_capabilities) = cli_fixture();
        let mut model = Cli::init(&cli, &cli_capabilities).model;
        let subscriptions = Cli::subscriptions(&cli, &cli_capabilities, &model);
        let subscription = subscriptions
            .iter()
            .next()
            .expect("an open CLI should desire its input");

        assert_eq!(
            subscription.source_descriptor::<StdinLines>(),
            Some(&StdinLines)
        );
        let message = subscription
            .map_source_event::<StdinLines>(SourceEvent::Item("print from stdin".to_owned()))
            .expect("the source type matches");
        assert!(matches!(
            message,
            CliMessage::Command(CliCommand::Print(text)) if text == "from stdin"
        ));

        let command = Cli::update(&cli, &cli_capabilities, &mut model, CliMessage::InputEnded);
        assert!(command.is_none());
        assert!(model.input_closed);
        assert_eq!(
            Cli::subscriptions(&cli, &cli_capabilities, &model)
                .iter()
                .count(),
            0
        );
    }

    #[test]
    fn cli_prints_the_time_request_reply() {
        let (cli, cli_capabilities) = cli_fixture();
        let mut model = Cli::init(&cli, &cli_capabilities).model;
        let time = DateTime::parse_from_rfc3339("2026-07-25T12:34:56Z")
            .expect("the fixture timestamp is valid")
            .with_timezone(&Utc);

        let command = Cli::update(
            &cli,
            &cli_capabilities,
            &mut model,
            CliMessage::TimeReceived(RequestOutcome::Replied(Some(time))),
        );
        assert_eq!(
            command
                .effect_intent::<PrintStdout>()
                .expect("a time reply should request stdout")
                .as_str(),
            "Current time: 2026-07-25 12:34:56 UTC\n"
        );
    }

    #[test]
    fn cli_reports_and_withdraws_failed_stdin() {
        let (cli, cli_capabilities) = cli_fixture();
        let mut model = Cli::init(&cli, &cli_capabilities).model;
        let subscriptions = Cli::subscriptions(&cli, &cli_capabilities, &model);
        let subscription = subscriptions
            .iter()
            .next()
            .expect("an open CLI should desire its input");
        let failure = StdinError::new(StdinErrorKind::Read, "fixture read failure");
        let message = subscription
            .map_source_event::<StdinLines>(SourceEvent::Failed(failure.clone()))
            .expect("the stdin source type matches");
        assert!(matches!(
            &message,
            CliMessage::InputFailed(error) if error == &failure
        ));

        let command = Cli::update(&cli, &cli_capabilities, &mut model, message);
        assert!(model.input_closed);
        assert_eq!(
            Cli::subscriptions(&cli, &cli_capabilities, &model)
                .iter()
                .count(),
            0
        );
        assert_eq!(
            command
                .effect_intent::<PrintStderr>()
                .expect("stdin failure should request stderr")
                .as_str(),
            "Failed to read standard input: standard-input Read error: fixture read failure\n"
        );
    }
}
