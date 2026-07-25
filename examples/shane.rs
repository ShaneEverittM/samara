use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Utc};
use futures_util::FutureExt;
use samara::prelude::*;
use serde::Deserialize;

enum Message {
    // A tick to drive our model.
    Tick,

    // Received when we are asked for our current time.
    GetTime(RequestInvocation<TimeServerProtocol, GetCurrentTime>),

    // Received when our time requests finish.
    TimeRetrieved(DateTime<Utc>),

    // Received when our time requests fail.
    FailedToGetTime(GetTimeError),
}

impl From<EffectOutcome<TimeResponse, HttpResponseError>> for Message {
    fn from(outcome: EffectOutcome<TimeResponse, HttpResponseError>) -> Self {
        use self::{EffectOutcome::*, Message::*};

        match outcome {
            Succeeded(response) => TimeRetrieved(response.data.iso),
            Failed(error) => FailedToGetTime(GetTimeError::Response(error)),
            Cancelled(reason) => FailedToGetTime(GetTimeError::Cancelled(reason)),
        }
    }
}

impl From<TimeServerProtocolMessage> for Message {
    fn from(value: TimeServerProtocolMessage) -> Self {
        match value {
            TimeServerProtocolMessage::GetCurrentTime(request) => Self::GetTime(request),
        }
    }
}

#[derive(Default)]
struct CliTimeModel {
    current_time: Option<DateTime<Utc>>,
}

struct CliTimeServer {
    http: EffectCapability<HttpRequest>,
    stdout: EffectCapability<PrintStdout>,
    stderr: EffectCapability<PrintStderr>,
}

impl Component for CliTimeServer {
    type Model = CliTimeModel;
    type Message = Message;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::default().with_command(Command::after(Duration::from_secs(1), Message::Tick))
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            // Got a tick, get the time and schedule the next tick.
            Message::Tick => {
                let get = HttpRequest::get("https://api.coinbase.com/v2/time")
                    .on_response()
                    .require_success()
                    .json::<TimeResponse>()
                    .into_command(&self.http);
                let tick = Command::after(Duration::from_secs(1), Message::Tick);

                Command::batch([get, tick])
            }

            // Someone wants to know our conception of time.
            Message::GetTime(request) => Command::reply(request.reply_to, model.current_time),

            // Got the time, save it and request a message be printed.
            Message::TimeRetrieved(time) => {
                model.current_time = Some(time);

                samara::println!(&self.stdout, "Got time: {time}")
            }

            // Couldn't get the time, request an error be printed.
            Message::FailedToGetTime(error) => {
                samara::eprintln!(&self.stderr, "Failed to get time: {error}")
            }
        }
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

    // Create a time-server component.
    let time_server = builder.component(
        ComponentId::new("TimeServer"),
        CliTimeServer {
            http,
            stdout,
            stderr,
        },
    );

    // Declare the existence of a Port on the runtime, which exposes the TimeServerProtocol.
    let port = builder.port::<TimeServerProtocol>(PortId::new("TimeServer"));

    // Bind the CliTimeServer as the implementation of the Port's protocol.
    builder.bind_port(&port, &time_server);

    // Build the program, this catches errors like unbound ports, etc.
    let program = builder.build()?;

    // Configure a runtime for real usage...
    let runtime = LiveRuntime::builder(program)
        // ...binding a built-in HTTP effect driver...
        .bind_http()
        // ...and a built-in stdio effect driver...
        .bind_stdio()
        // ...and build it, similar to program finding configuration errors.
        .build()?;

    // We are now "outside" Samara-land, so we can ask for a "handle" as a point
    // of ingress. This allows normal tokio tasks to interact with Components.
    let time_server = runtime.port_handle(&port)?;

    // We've now set up the program, runtime, and ingress points, so start the runtime.
    let mut runtime_task = runtime.spawn();

    // Ask the time-server for its current state, while keeping an immediately
    // faulting runtime visible instead of waiting for a later shutdown call.
    let current_time = tokio::select! {
        result = runtime_task.run_forever() => {
            result?;
            return Ok(());
        }
        result = tokio::time::sleep(Duration::from_secs(2)).then(|_| time_server.request(GetCurrentTime)) => result?,
    };
    println!("Time reported through the Port: {current_time:?}");

    // Signal handling remains ordinary host policy. Cancelling the
    // run_forever observation leaves runtime_task owning every Samara task, so
    // the host can still select Drain or Cancel explicitly afterward.
    tokio::select! {
        result = runtime_task.run_forever() => {
            result?;
            return Ok(());
        }
        signal = tokio::signal::ctrl_c() => signal?,
    }
    runtime_task.shutdown(Shutdown::Cancel).await?;

    Ok(())
}
