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

    // Received when we are asked for our current time.
    GetTime(RequestInvocation<TimeServerProtocol, GetCurrentTime>),

    // Received when our time requests finish.
    TimeRetrieved(DateTime<Utc>),

    // Received when our time requests fail.
    FailedToGetTime(GetTimeError),
}

impl From<EffectOutcome<TimeResponse, HttpResponseError>> for HttpTimeMessage {
    fn from(outcome: EffectOutcome<TimeResponse, HttpResponseError>) -> Self {
        use self::{EffectOutcome::*, HttpTimeMessage::*};

        match outcome {
            Succeeded(response) => TimeRetrieved(response.data.iso),
            Failed(error) => FailedToGetTime(GetTimeError::Response(error)),
            Cancelled(reason) => FailedToGetTime(GetTimeError::Cancelled(reason)),
        }
    }
}

impl From<TimeServerProtocolMessage> for HttpTimeMessage {
    fn from(value: TimeServerProtocolMessage) -> Self {
        match value {
            TimeServerProtocolMessage::GetCurrentTime(request) => Self::GetTime(request),
        }
    }
}

#[derive(Default)]
struct HttpTimeModel {
    current_time: Option<DateTime<Utc>>,
}

/// HTTP-backed implementation of the provider-neutral time protocol.
struct HttpTimeServer {
    http: EffectCapability<HttpRequest>,
    stderr: EffectCapability<PrintStderr>,
}

impl Component for HttpTimeServer {
    type Model = HttpTimeModel;
    type Message = HttpTimeMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::default().with_command(Command::after(
            Duration::from_secs(1),
            HttpTimeMessage::Tick,
        ))
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            // Got a tick, get the time and schedule the next tick.
            HttpTimeMessage::Tick => {
                let get = HttpRequest::get("https://api.coinbase.com/v2/time")
                    .on_response()
                    .require_success()
                    .json::<TimeResponse>()
                    .into_command(&self.http);
                let tick = Command::after(Duration::from_secs(1), HttpTimeMessage::Tick);

                Command::batch([get, tick])
            }

            // Someone wants to know our conception of time.
            HttpTimeMessage::GetTime(request) => {
                Command::reply(request.reply_to, model.current_time)
            }

            // Got the time, save it. Consumers decide how to present it.
            HttpTimeMessage::TimeRetrieved(time) => {
                model.current_time = Some(time);
                Command::none()
            }

            // Couldn't get the time, request an error be printed.
            HttpTimeMessage::FailedToGetTime(error) => {
                samara::eprintln!(&self.stderr, "Failed to get time: {error}")
            }
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

impl From<RequestOutcome<Option<DateTime<Utc>>>> for CliMessage {
    fn from(outcome: RequestOutcome<Option<DateTime<Utc>>>) -> Self {
        Self::TimeReceived(outcome)
    }
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
struct Cli {
    input: SourceCapability<StdinLines>,
    time_server: Port<TimeServerProtocol>,
    stdout: EffectCapability<PrintStdout>,
    stderr: EffectCapability<PrintStderr>,
}

impl Component for Cli {
    type Model = CliModel;
    type Message = CliMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::default().with_command(samara::println!(
            &self.stdout,
            "Commands: `print <text>` or `time`"
        ))
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            CliMessage::Command(CliCommand::Print(text)) => {
                samara::println!(&self.stdout, "{text}")
            }
            CliMessage::Command(CliCommand::GetTime) => {
                Command::request(self.time_server.clone(), GetCurrentTime)
            }
            CliMessage::InvalidCommand(command) => samara::eprintln!(
                &self.stderr,
                "Unknown command `{command}`; use `print <text>` or `time`"
            ),
            CliMessage::InputEnded => {
                model.input_closed = true;
                Command::none()
            }
            CliMessage::InputFailed(error) => {
                model.input_closed = true;
                samara::eprintln!(&self.stderr, "Failed to read standard input: {error}")
            }
            CliMessage::TimeReceived(RequestOutcome::Replied(Some(time))) => {
                samara::println!(&self.stdout, "Current time: {time}")
            }
            CliMessage::TimeReceived(RequestOutcome::Replied(None)) => {
                samara::println!(&self.stdout, "The time server has not received a value yet")
            }
            CliMessage::TimeReceived(RequestOutcome::Failed(error)) => {
                samara::eprintln!(&self.stderr, "Time request failed: {error:?}")
            }
            CliMessage::TimeReceived(RequestOutcome::TimedOut) => {
                samara::eprintln!(&self.stderr, "Time request timed out")
            }
            CliMessage::TimeReceived(RequestOutcome::Cancelled) => {
                samara::eprintln!(&self.stderr, "Time request was cancelled")
            }
        }
    }

    fn subscriptions(&self, model: &Self::Model) -> Subscriptions<Self::Message> {
        if model.input_closed {
            return Subscriptions::none();
        }

        Subscriptions::one(Subscription::source_with(
            &self.input,
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

protocol! {
    type TimeServerProtocol => enum TimeServerProtocolMessage {
        GetCurrentTime -> Option<DateTime<Utc>>,
    }
}

#[cfg(unix)]
#[tokio::main]
async fn main() -> Result<()> {
    // Start building a Samara program, which is a declaration of the topology.
    let mut builder = Program::builder();

    // Declare every world boundary before constructing Components. These inert
    // capabilities are both the dependency declarations and the only way the
    // Component can issue the corresponding Effects.
    let http = builder.effect::<HttpRequest>();
    let stdout = builder.effect::<PrintStdout>();
    let stderr = builder.effect::<PrintStderr>();
    let cli_input = builder.source::<StdinLines>();

    // A Port names the provider-neutral time service consumed by the CLI.
    let time_port = builder.port::<TimeServerProtocol>(PortId::new(TIME_SERVER_PORT));

    // The concrete provider happens to obtain time over HTTP.
    let http_time_server = builder.component(
        ComponentId::new("http-time-server"),
        HttpTimeServer {
            http,
            stderr: stderr.clone(),
        },
    );

    // Program assembly chooses which concrete Component provides that service.
    builder.bind_port(&time_port, &http_time_server);

    // Raw terminal lines cross the runtime boundary as an ongoing Source. The
    // CLI owns parsing, presentation, and its dependency on the time service.
    builder.component(
        ComponentId::new("cli"),
        Cli {
            input: cli_input.clone(),
            time_server: time_port,
            stdout,
            stderr,
        },
    );

    // Build the program, this catches errors like unbound ports, etc.
    let program = builder.build()?;

    // Configure a runtime for real usage...
    let runtime = LiveRuntime::builder(program)
        // ...binding a built-in HTTP effect driver...
        .bind_http()
        // ...and a built-in stdio effect driver...
        .bind_stdio()
        // ...and binding the declared command Source to process stdin.
        .bind_stdin(&cli_input)
        // ...and build it, similar to program finding configuration errors.
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
mod tests {
    use super::*;

    fn cli_fixture() -> Cli {
        let mut builder = Program::builder();
        Cli {
            input: builder.source::<StdinLines>(),
            time_server: builder.port(PortId::new(TIME_SERVER_PORT)),
            stdout: builder.effect::<PrintStdout>(),
            stderr: builder.effect::<PrintStderr>(),
        }
    }

    #[test]
    fn cli_commands_declare_print_and_time_request_intent() {
        let cli = cli_fixture();
        let mut model = cli.init().model;

        let print = cli.update(
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

        let time = cli.update(&mut model, CliMessage::Command(CliCommand::GetTime));
        let requests = time.request_intents::<GetCurrentTime>();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, &PortId::new(TIME_SERVER_PORT));
    }

    #[test]
    fn cli_subscription_parses_input_and_withdraws_after_eof() {
        let cli = cli_fixture();
        let mut model = cli.init().model;
        let subscriptions = cli.subscriptions(&model);
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

        let command = cli.update(&mut model, CliMessage::InputEnded);
        assert!(command.is_none());
        assert!(model.input_closed);
        assert_eq!(cli.subscriptions(&model).iter().count(), 0);
    }

    #[test]
    fn cli_prints_the_time_request_reply() {
        let cli = cli_fixture();
        let mut model = cli.init().model;
        let time = DateTime::parse_from_rfc3339("2026-07-25T12:34:56Z")
            .expect("the fixture timestamp is valid")
            .with_timezone(&Utc);

        let command = cli.update(
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
        let cli = cli_fixture();
        let mut model = cli.init().model;
        let subscriptions = cli.subscriptions(&model);
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

        let command = cli.update(&mut model, message);
        assert!(model.input_closed);
        assert_eq!(cli.subscriptions(&model).iter().count(), 0);
        assert_eq!(
            command
                .effect_intent::<PrintStderr>()
                .expect("stdin failure should request stderr")
                .as_str(),
            "Failed to read standard input: standard-input Read error: fixture read failure\n"
        );
    }
}
