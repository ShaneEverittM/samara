use samara::prelude::*;
use std::{sync::Arc, time::Duration};
use tokio::time::Instant;

struct Greeter;

#[derive(Clone)]
struct Greet(pub String);

impl Request<Greeter> for Greet {
    type Reply = String;

    fn into_msg(self) -> <Greeter as Actor>::Msg {
        MyMessage::Request(self.0)
    }
}

enum MyMessage {
    Request(String),

    GreetingReady { message: String },
}

enum MyCommand {
    WaitToGreet { name: String, duration: Duration },
    Print { message: String },
    Reply { message: String },
}

impl Actor for Greeter {
    type Msg = MyMessage;
    type Cmd = MyCommand;
    type Driver = MyDriver;
    type DriverContext = ();

    fn update(&mut self, msg: Self::Msg, ctx: &UpdateContext) -> Vec<Self::Cmd> {
        match msg {
            MyMessage::Request(name) => {
                ctx.claim_reply();
                vec![MyCommand::WaitToGreet {
                    name,
                    duration: Duration::from_millis(500),
                }]
            }
            MyMessage::GreetingReady { message } => {
                ctx.claim_reply();
                vec![
                    MyCommand::Print {
                        message: message.clone(),
                    },
                    MyCommand::Reply { message },
                ]
            }
        }
    }

    fn effect_driver(_: ()) -> Self::Driver
    where
        Self: Sized,
    {
        MyDriver
    }
}

struct MyDriver;

impl EffectDriver<MyCommand> for MyDriver {
    fn run(&self, issued: IssuedCmd<MyCommand>, ctx: &EffectContext) -> EffectRun {
        match issued.cmd {
            MyCommand::WaitToGreet { name, duration } => EffectRun::system_effect(Sleep(duration))
                .on_ok(move |_| {
                    vec![Envelope::with_meta(
                        issued.origin,
                        MyMessage::GreetingReady {
                            message: format!("Hello, {}!", name),
                        },
                        issued.meta.clone(),
                    )]
                })
                .into_run(),
            MyCommand::Print { message } => {
                EffectRun::side_effect_future(async move { println!("greet -> {message}") })
            }
            MyCommand::Reply { message } => {
                let effect_ctx = ctx.clone();
                let meta = issued.meta.clone();
                EffectRun::side_effect_future(async move {
                    let _ = effect_ctx.reply_from_meta(&meta, message);
                })
            }
        }
    }
}

#[tokio::main]
async fn main() {
    let mut runtime = Runtime::new(128, Arc::new(TokioBackend));
    let actor = runtime
        .register_actor(ActorId(1), Greeter, Greeter::effect_driver(()))
        .expect("unique id");

    actor
        .tell_request(Greet("Shane".to_string()))
        .await
        .expect("tell should enqueue");

    let ask_task = tokio::spawn({
        let actor = actor.clone();
        async move { actor.ask_request(Greet("Samara".to_string())).await }
    });

    let now = Instant::now();
    let exit = runtime.run_until_predicate(|| ask_task.is_finished()).await;
    let elapsed = now.elapsed();
    println!("Ran for {elapsed:?}");

    assert_eq!(exit, RunUntilExit::ConditionMet);

    let response = ask_task
        .await
        .expect("ask task should not panic")
        .expect("ask should resolve");
    println!("ask -> {response}");

    let now = Instant::now();
    let exit = runtime.run_until_idle().await;
    let elapsed = now.elapsed();
    println!("Ran for another {elapsed:?}");
    assert_eq!(exit, RunUntilExit::Idle);
}
