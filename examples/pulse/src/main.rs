use samara::prelude::*;

use pulse::journal::{Journal, JournalMessage};

struct Monitor;

#[derive(Clone, samara::Io)]
struct Io {
    input: Source<StdinLines>,
    output: Effect<PrintStdout>,
}

struct Model;

enum Message {
    Input(String),
    JournalResponse(Vec<String>),
}

impl Component for Monitor {
    type Io = Io;

    type Connections = Connector<JournalMessage>;

    type Configuration = ();

    type Model = Model;

    type Message = Message;

    fn init(_: &Context<Self>) -> Init<Self::Model, Self::Message> {
        Init::new(Model)
    }

    fn update(
        cx: &Context<Self>,
        _: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        match message {
            Message::Input(s) => {
                Command::request(&cx.to, move |reply| {
                    JournalMessage::Recent {
                        count: if s == "5" { 5 } else { 10 },
                        reply,
                    }
                })
                .on_reply(|recent| Message::JournalResponse(recent.unwrap()))
                // This would handle a timeout or an abandoned reply.
                .on_failure(|_| todo!())
            }
            Message::JournalResponse(r) => samara::println!(&cx.io.output, "Got response: {:?}", r),
        }
    }

    fn subscriptions(cx: &Context<Self>, _model: &Self::Model) -> Subscriptions<Self::Message> {
        Subscriptions::one(Subscription::source_with(
            &cx.io.input,
            StdinLines::new(),
            |event| {
                match event {
                    SourceEvent::Item(s) => Message::Input(s),
                    // This would handle EOF or errors.
                    _ => todo!(),
                }
            },
        ))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut builder = Program::builder();

    let journal = builder.add_assembly::<Journal>(());
    let monitor = builder.add_component::<Monitor>(journal.connector());

    let program = builder.build()?;

    println!("{}", program.manifest());

    let runtime = Runtime::builder(program)
        .supply(&journal.io.file, FileDriver::new("data/pulse.journal"))
        .supply_default(&(journal.io.clock, monitor.io.input, monitor.io.output))
        .build()?;

    runtime.run().await?;

    Ok(())
}
