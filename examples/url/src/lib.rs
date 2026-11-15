use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use samara::prelude::*;
use samara::{HttpResponsePipeline, eprintln, println};
use url::Url;

#[derive(Debug)]
enum UrlCommand {
    Url(Url),
    Status,
    Clear,
}

enum InputEvent {
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
                Err(_) => {
                    let url = String::from("https://") + url;
                    if let Ok(url) = Url::parse(&url) {
                        return Self::Command(UrlCommand::Url(url));
                    }

                    Self::Malformed(format!("Invalid URL: {}", url))
                }
            },
            (Some("status"), None, None) => Self::Command(UrlCommand::Status),
            (Some("clear"), None, None) => Self::Command(UrlCommand::Clear),
            _ => Self::Malformed("usage: url <address> | status | clear".into()),
        }
    }
}

enum Message {
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
enum CheckResult {
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

enum Status {
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
        result: CheckResult,
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

struct Checker;

struct CheckerConfig {}

/// Everything the URL checker can reach in the world. [`program`] returns
/// these so the host can supply a live Driver for each.
#[derive(Clone, Capabilities)]
pub struct UrlIo {
    pub stdin: Source<StdinLines>,
    pub stdout: Effect<PrintStdout>,
    pub stderr: Effect<PrintStderr>,
    pub http: Effect<HttpRequest>,
}

impl Checker {
    const DELAY: Duration = Duration::from_millis(500);
}

impl Component for Checker {
    type Io = UrlIo;
    type Config = CheckerConfig;
    type Model = Status;
    type Message = Message;

    fn init(_: &CheckerConfig, _io: &Self::Io) -> Init<Status, Message> {
        Init::new(Status::Idle { revision: 0 })
    }

    fn update(
        _config: &CheckerConfig,
        io: &Self::Io,
        model: &mut Status,
        message: Message,
    ) -> Command<Message> {
        match message {
            Message::Input(event) => {
                let command = match event {
                    InputEvent::Command(command) => command,
                    InputEvent::Malformed(e) => return eprintln!(&io.stderr, "{e}"),
                    InputEvent::Error(e) => return eprintln!(&io.stderr, "{e:?}"),
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
                        let next = model.revision().checked_add(1).expect("revision overflow");

                        *model = Status::Pending {
                            url: Arc::clone(&url),
                            revision: next,
                        };

                        let log =
                            println!(&io.stdout, "Waiting {:?} before checking...", Self::DELAY);
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
                        println!(&io.stdout, "Cleared")
                    }

                    UrlCommand::Status => match &model {
                        Status::Idle { .. } => {
                            println!(
                                &io.stdout,
                                "URL: {}\nState: {}\nLast result: {}", "none", "idle", "none"
                            )
                        }
                        Status::Pending { url, .. } => {
                            println!(
                                &io.stdout,
                                "URL: {}\nState: {}\nLast result: {}", url, "waiting", "none"
                            )
                        }
                        Status::InFlight { url, .. } => {
                            println!(
                                &io.stdout,
                                "URL: {}\nState: {}\nLast result: {}", url, "checking", "none"
                            )
                        }
                        Status::Finished { url, result, .. } => {
                            println!(
                                &io.stdout,
                                "URL: {}\nState: {}\nLast result: {}", url, "idle", result
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
                let log = println!(&io.stdout, "Checking {}", url);
                let request =
                    request.into_command_with(&io.http, move |r| Message::CheckFinished {
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
                    EffectOutcome::Succeeded(response) => CheckResult::Response(response.status()),
                    EffectOutcome::Failed(e) => CheckResult::Error(e.to_string()),
                };
                *model = Status::Finished {
                    revision,
                    url: Arc::clone(&url),
                    result: result.clone(),
                };
                println!(&io.stdout, "{}: {}", url, result)
            }
        }
    }

    fn subscriptions(_config: &CheckerConfig, io: &Self::Io, _: &Status) -> Subscriptions<Message> {
        Subscriptions::one(Subscription::source_with(
            &io.stdin,
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

/// Assembles the URL checker for live or controlled execution, returning the
/// Program and the capabilities a live host supplies Drivers for.
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let (program, io) = urlchecker::program()?;
/// let runtime = samara::Runtime::builder(program)
///     .supply(&io.stdin, samara::StdinDriver)
///     .supply(&io.stdout, samara::StdoutDriver::new())
///     .supply(&io.stderr, samara::StderrDriver::new())
///     .supply(&io.http, samara::HttpDriver::new())
///     .build()?;
/// // Inside Tokio, call runtime.spawn() and use its RuntimeTask for shutdown.
/// # Ok(())
/// # }
/// ```
pub fn program() -> Result<(Program, UrlIo), ProgramBuildError> {
    let (program, checker) = assemble()?;
    Ok((program, checker.io))
}

// Tests retain the Checker's address and capabilities without exposing internals.
fn assemble() -> Result<(Program, InstalledComponent<Checker>), ProgramBuildError> {
    let mut builder = Program::builder();

    let checker = builder
        .component::<Checker>(CheckerConfig {})
        .finish()
        .expect("component registration");

    Ok((builder.build()?, checker))
}

#[cfg(test)]
mod tests;
