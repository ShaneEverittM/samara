#![allow(unused, dead_code)]
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::Deserialize;
use tokio::select;
use tokio::time::interval;

#[derive(Deserialize)]
struct TimeResponseData {
    iso: DateTime<Utc>,
}

#[derive(Deserialize)]
struct TimeResponse {
    data: TimeResponseData,
}

struct TimeServer {
    current_time: Mutex<Option<DateTime<Utc>>>,
    client: reqwest::Client,
}

impl TimeServer {
    fn new() -> Self {
        Self {
            current_time: Mutex::default(),
            client: reqwest::Client::new(),
        }
    }

    async fn run(self: Arc<Self>) {
        let mut timer = interval(Duration::from_secs(1));
        loop {
            timer.tick().await;

            let Ok(response) = self
                .client
                .get("https://api.coinbase.com/v2/time")
                .send()
                .await
            else {
                continue;
            };

            let time = response
                .json::<TimeResponse>()
                .await
                .ok()
                .map(|body| body.data.iso);

            *self.current_time.lock() = time;
        }
    }

    fn get_time(&self) -> Option<DateTime<Utc>> {
        self.current_time.lock().clone()
    }
}

struct Cli {
    time_server: Arc<TimeServer>,
}

impl Cli {
    fn new(time_server: Arc<TimeServer>) -> Self {
        Cli { time_server }
    }

    async fn run(self) {
        let stdin = tokio::io::stdin();
        loop {}
    }
}

#[tokio::main]
async fn main() {
    let server = Arc::new(TimeServer::new());
    let server_handle = Arc::clone(&server).run();
    let server_task = tokio::spawn(server_handle);
}
