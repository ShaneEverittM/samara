use anyhow::Result;
use samara::prelude::*;
use serde::Deserialize;
use std::time::Duration;

type Time = chrono::DateTime<chrono::Utc>;

enum Message {
    GetTime,
    TimeRetrieved(Time),
    FailedToGetTime(GetTimeError),
}

impl From<EffectOutcome<TimeResponse, HttpResponseError>> for Message {
    fn from(outcome: EffectOutcome<TimeResponse, HttpResponseError>) -> Self {
        match outcome {
            EffectOutcome::Succeeded(response) => Message::TimeRetrieved(response.data.iso),
            EffectOutcome::Failed(error) => Message::FailedToGetTime(error.into()),
            EffectOutcome::Cancelled(reason) => {
                Message::FailedToGetTime(GetTimeError::Cancelled(reason))
            }
        }
    }
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
                let get_time = HttpRequest::get("https://api.coinbase.com/v2/time")
                    .on_response()
                    .require_success()
                    .json::<TimeResponse>()
                    .into_command(Message::from);
                let tick = Command::after(Duration::from_secs(1), Message::GetTime);
                Command::batch([get_time, tick])
            }

            Message::TimeRetrieved(time) => {
                model.current_time = Some(time);
                samara::println!("Got time: {time}")
            }
            Message::FailedToGetTime(error) => {
                samara::eprintln!("Failed to get time from server: {error}")
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
    iso: Time,
}

#[derive(Deserialize)]
struct TimeResponse {
    data: TimeResponseData,
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut builder = Program::builder();
    builder.component(ComponentId::new("TimeServer"), CliTimeServer);
    let program = builder.build()?;

    let runtime = LiveRuntime::builder(program)
        .bind_http()
        .bind_stdio()
        .build()?;
    let handle = runtime.spawn();

    // Note: should have a run_forever capability.
    tokio::time::sleep(Duration::from_secs(10)).await;
    handle.shutdown(Shutdown::Cancel).await?;

    Ok(())
}
