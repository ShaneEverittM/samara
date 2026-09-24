use std::sync::Arc;
use std::time::Duration;
use std::fmt;

use samara::prelude::*;
use samara::{HttpResponsePipeline, eprintln, println};
use url::Url;

#[derive(Debug)]
pub enum UrlCommand {
    Url(Url),
    Status,
    Clear,
}

pub enum InputEvent {
    Command(UrlCommand),
    Error(StdinError),
    Malformed(String),
    Eof,
}

impl InputEvent {
    fn parse(line: &str) -> InputEvent {
        let mut words = line.split_whitespace();
        match (words.next(), words.next(), words.next()) {
            (Some("url"), Some(url), None) => match Url::parse(url) {
                Ok(url) => Self::Command(UrlCommand::Url(url)),
                Err(_) => Self::Malformed(format!("Invalid URL: {}", url)),
            },
            (Some("status"), None, None) => Self::Command(UrlCommand::Status),
            (Some("clear"), None, None) => Self::Command(UrlCommand::Clear),
            _ => Self::Malformed(format!("Invalid input: {}", line)),
        }
    }
}

pub enum Message {
    Input(InputEvent),
    CheckDue {
        revision: u64,
        url: Arc<str>,
        request: HttpResponsePipeline<HttpResponse, HttpError>,
    },
    CheckFinished {
        revision: u64,
        url: Arc<str>,
        outcome: EffectOutcome<HttpResponse, HttpError>,
    },
}

#[derive(Clone)]
pub enum CheckResult {
    Response(StatusCode),
    Error(String),
}

impl fmt::Display for CheckResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CheckResult::Response(code) => write!(f, "{code}"),
            CheckResult::Error(e) => write!(f, "Check failed: {e}"),
        }
    }
}

pub enum Status {
    Idle {
        revision: u64,
    },
    Pending {
        url: Arc<str>,
        revision: u64,
    },
    InFlight {
        url: Arc<str>,
        revision: u64,
    },
    Finished {
        revision: u64,
        url: Arc<str>,
        result: Option<CheckResult>,
    },
}

impl Status {
    fn revision(&self) -> u64 {
        let (Self::Idle { revision, .. }
        | Self::Pending { revision, .. }
        | Self::InFlight { revision, .. }
        | Self::Finished { revision, .. }) = self;

        *revision
    }
}

pub struct Checker {
    input: SourceCapability<StdinLines>,
    stdout: EffectCapability<PrintStdout>,
    stderr: EffectCapability<PrintStderr>,
    http: EffectCapability<HttpRequest>,
}

impl Checker {
    pub const DELAY: Duration = Duration::from_millis(500);
}

impl Component for Checker {
    type Model = Status;
    type Message = Message;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(Status::Idle { revision: 0 })
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            Message::Input(event) => {
                let command = match event {
                    InputEvent::Command(command) => command,
                    InputEvent::Malformed(e) => return eprintln!(&self.stderr, "{e:?}"),
                    InputEvent::Error(e) => return eprintln!(&self.stderr, "{e:?}"),
                    // This should return stop, and the runtime could perhaps exit
                    // if all components have stopped.
                    InputEvent::Eof => return Command::none(),
                };

                match command {
                    UrlCommand::Url(url) => {
                        let url = Arc::from(url.as_str());
                        let request = HttpRequest::get(Arc::clone(&url))
                            .on_response()
                            .follow_redirects(5);
                        let next = model.revision() + 1;

                        *model = Status::Pending {
                            url: Arc::clone(&url),
                            revision: next,
                        };

                        let log = println!(&self.stdout, "Waiting 500ms before checking...");
                        let check = Command::after(
                            Self::DELAY,
                            Message::CheckDue {
                                url: Arc::clone(&url),
                                revision: next,
                                request,
                            },
                        );

                        Command::batch([log, check])
                    }

                    UrlCommand::Clear => {
                        let revision = model.revision();
                        *model = Status::Idle {
                            revision: revision.checked_add(1).expect("revision overflow"),
                        };
                        println!(&self.stdout, "Cleared")
                    }

                    UrlCommand::Status => match &model {
                        Status::Idle { .. } => {
                            println!(
                                &self.stdout,
                                "URL: {}\nState: {}\nLast result: {}", "none", "idle", "none"
                            )
                        }
                        Status::Pending { url, .. } => {
                            println!(
                                &self.stdout,
                                "URL: {}\nState: {}\nLast result: {}", url, "waiting", "none"
                            )
                        }
                        Status::InFlight { url, .. } => {
                            println!(
                                &self.stdout,
                                "URL: {}\nState: {}\nLast result: {}", url, "checking", "none"
                            )
                        }
                        Status::Finished { url, result, .. } => {
                            println!(
                                &self.stdout,
                                "URL: {}\nState: {}\nLast result: {}",
                                url,
                                "idle",
                                result
                                    .as_ref()
                                    .map(|it| it.to_string())
                                    .unwrap_or("cancelled".into())
                            )
                        }
                    },
                }
            }

            Message::CheckDue {
                url,
                request,
                revision,
            } => {
                // A debounced check that has been superseded just arrived.
                if model.revision() != revision {
                    return Command::none();
                }

                *model = Status::InFlight {
                    url: Arc::clone(&url),
                    revision,
                };
                let log = println!(&self.stdout, "Checking {}", url);
                let request = request.into_command_with(&self.http, move |r| Message::CheckFinished {
                    url,
                    revision,
                    outcome: r,
                });
                Command::batch([log, request])
            }

            Message::CheckFinished {
                url,
                revision,
                outcome,
            } => {
                // The result of a check that has been superseded just arrived.
                if model.revision() != revision {
                    return Command::none();
                }

                let result = match &outcome {
                    EffectOutcome::Succeeded(response) => {
                        Some(CheckResult::Response(response.status()))
                    }
                    EffectOutcome::Failed(e) => Some(CheckResult::Error(e.to_string())),
                    EffectOutcome::Cancelled(_) => None,
                };
                *model = Status::Finished {
                    revision,
                    url: Arc::clone(&url),
                    result: result.clone(),
                };
                println!(
                    &self.stdout,
                    "{}: {}",
                    url,
                    result
                        .map(|it| it.to_string())
                        .unwrap_or("cancelled".into())
                )
            }
        }
    }

    fn subscriptions(&self, _: &Self::Model) -> Subscriptions<Self::Message> {
        Subscriptions::one(Subscription::source_with(
            &self.input,
            SubscriptionId::new("commands"),
            StdinLines::new(),
            |event| {
                Message::Input(match event {
                    SourceEvent::Item(line) => InputEvent::parse(&line),
                    SourceEvent::Ended => InputEvent::Eof,
                    SourceEvent::Failed(error) => InputEvent::Error(error),
                })
            },
        ))
    }
}

pub fn program() -> Result<
    (
        Program,
        (ComponentRef<Checker>,),
        (SourceCapability<StdinLines>,),
    ),
    ProgramBuildError,
> {
    let mut builder = Program::builder();

    let http = builder.effect::<HttpRequest>();
    let stdout = builder.effect::<PrintStdout>();
    let stderr = builder.effect::<PrintStderr>();
    let input = builder.source::<StdinLines>();

    let checker = builder.component(
        ComponentId::new("Url Checker"),
        Checker {
            input: input.clone(),
            stdout,
            stderr,
            http,
        },
    );

    Ok((builder.build()?, (checker,), (input,)))
}
