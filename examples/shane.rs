use anyhow::Result;
use futures_util::{FutureExt, TryFutureExt};
use reqwest::{Client, Response};
use samara::prelude::*;
use serde::Deserialize;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::time::Duration;

type Time = chrono::DateTime<chrono::Utc>;

enum Message {
    GetTime,
    TimeRetrieved(Time),
    FailedToGetTime,
    OutputFinished(EffectOutcome<(), std::convert::Infallible>),
}

#[derive(Default)]
struct CliTimeModel {
    current_time: Option<Time>,
}

struct CliTimeServer;

impl Component for CliTimeServer {
    type Model = CliTimeModel;
    type Message = Message;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        // Note: perhaps `Init::default`?
        Init::new(CliTimeModel::default())
            .with_command(Command::after(Duration::from_secs(1), Message::GetTime))
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            Message::GetTime => {
                let get_time = Command::effect(GetTime, |outcome| match outcome {
                    EffectOutcome::Succeeded(data) => Message::TimeRetrieved(data),
                    _ => Message::FailedToGetTime,
                });
                let tick = Command::after(Duration::from_secs(1), Message::GetTime);
                Command::batch([get_time, tick])
            }

            Message::TimeRetrieved(time) => {
                let output = PrintStdout::line(format!("Got time: {time}"));
                model.current_time = Some(time);
                Command::effect(output, Message::OutputFinished)
            }
            Message::FailedToGetTime => Command::effect(
                PrintStderr::line("Failed to get time from server"),
                Message::OutputFinished,
            ),
            Message::OutputFinished(_outcome) => Command::none(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GetTime;

#[derive(Debug)]
struct GetTimeError {
    message: String,
}

impl GetTimeError {
    fn from_err<E>(error: E) -> GetTimeError
    where
        E: Display,
    {
        Self {
            message: format!("{}", error),
        }
    }
}

impl Display for GetTimeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl Error for GetTimeError {}

impl EffectDescriptor for GetTime {
    type Output = Time;
    type Error = GetTimeError;
}

impl EffectDriver<GetTime> for CliTimeServer {
    fn execute(&self, _: GetTime) -> BoxFuture<Result<Time, GetTimeError>> {
        #[derive(Deserialize)]
        struct TimeResponseData {
            iso: Time,
        }

        #[derive(Deserialize)]
        struct TimeResponse {
            data: TimeResponseData,
        }

        // This is neat we can do this, but perhaps it should a built-in, runtime-owned effect
        // descriptor for HTTP like we have for TCP. Though custom effects are just first-party
        // effects that we haven't built yet.
        Client::new()
            .get("https://api.coinbase.com/v2/time")
            .send()
            .and_then(Response::json::<TimeResponse>)
            .map_ok(|response| response.data.iso)
            .map_err(GetTimeError::from_err)
            .boxed()
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut builder = Program::builder();
    builder.component(ComponentId::new("TimeServer"), CliTimeServer);
    let program = builder.build()?;

    let runtime = LiveRuntime::builder(program)
        .bind_effect::<GetTime, _>(CliTimeServer)
        .bind_stdio()
        .build()?;
    let handle = runtime.spawn();

    // Note: should have a run_forever capability.
    tokio::time::sleep(Duration::from_secs(10)).await;
    handle.shutdown(Shutdown::Cancel).await?;

    Ok(())
}
