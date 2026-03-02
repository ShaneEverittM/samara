use std::{sync::Arc, time::Duration};

use samara::prelude::*;

struct GreetingPort;

impl Port for GreetingPort {
    type Req = String;
    type Res = String;
}

struct Greeter;

enum MyMessage {
    Request(String),

    GreetingReady {
        message: String,
        reply_to: ReplyToken<String>,
    },
}

enum MyCommand {
    WaitToGreet {
        name: String,
        duration: Duration,
        reply_to: ReplyToken<String>,
    },
    Print {
        message: String,
    },
    Reply {
        message: String,
        reply_to: ReplyToken<String>,
    },
}

impl Actor for Greeter {
    type Msg = MyMessage;
    type Cmd = MyCommand;
    type Driver = MyDriver;
    type DriverContext = ();

    fn update(&mut self, msg: Self::Msg, ctx: &UpdateContext) -> Vec<Self::Cmd> {
        match msg {
            MyMessage::Request(name) => {
                let Some(reply_to) = ctx.reply_token::<String>() else {
                    return Vec::new();
                };
                vec![MyCommand::WaitToGreet {
                    name,
                    duration: Duration::from_millis(500),
                    reply_to,
                }]
            }
            MyMessage::GreetingReady { message, reply_to } => vec![
                MyCommand::Print {
                    message: message.clone(),
                },
                MyCommand::Reply { message, reply_to },
            ],
        }
    }

    fn effect_driver(_: ()) -> Self::Driver
    where
        Self: Sized,
    {
        MyDriver
    }
}

impl PortHandler<GreetingPort> for Greeter {
    fn request(req: String) -> Self::Msg {
        MyMessage::Request(req)
    }
}

struct MyDriver;

impl EffectDriver<MyCommand> for MyDriver {
    fn run(&self, issued: IssuedCmd<MyCommand>, ctx: &EffectContext) -> EffectRun {
        match issued.cmd {
            MyCommand::WaitToGreet {
                name,
                duration,
                reply_to,
            } => EffectRun::system_effect(Sleep(duration))
                .on_ok(move |_| {
                    vec![Envelope::new(
                        issued.origin,
                        MyMessage::GreetingReady {
                            message: format!("Hello, {}!", name),
                            reply_to,
                        },
                    )]
                })
                .into_run(),
            MyCommand::Print { message } => {
                EffectRun::side_effect_future(async move { println!("greet -> {message}") })
            }
            MyCommand::Reply { message, reply_to } => {
                let runtime = ctx.runtime().clone();
                EffectRun::side_effect_future(async move {
                    let _ = runtime.reply(reply_to, message);
                })
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let mut runtime = Runtime::new(128, Arc::new(TokioBackend));
    runtime
        .register_actor(ActorId(1), Greeter, Greeter::effect_driver(()))
        .expect("unique id");
    let port = runtime
        .register_port::<GreetingPort, Greeter>()
        .expect("bind greeting port to provider actor");

    port.tell("Shane".to_string())
        .await
        .expect("tell should enqueue");

    let ask_task = tokio::spawn({
        let port = port.clone();
        async move { port.ask("Samara".to_string()).await }
    });

    let exit = runtime.run_until_predicate(|| ask_task.is_finished()).await;
    assert_eq!(exit, RunUntilExit::ConditionMet);

    let response = ask_task
        .await
        .expect("ask task should not panic")
        .expect("ask should resolve");
    println!("ask -> {response}");

    let exit = runtime.run_until_idle().await;
    assert_eq!(exit, RunUntilExit::Idle);
}
