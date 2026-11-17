use samara::prelude::*;

// This is a "library assembly", that is a subsection of a Program (discussed later)
// designed to be embedded in another program.
use pulse::journal::{Journal, JournalMessage};

// This is the component of our application.
struct Monitor;

// What it can reach in the world, by name.
#[derive(Clone, samara::Io)]
struct Io {
    input: Source<StdinLines>,
    output: Effect<PrintStdout>,
}

// Stateless for now.
struct Model;

// Messages our model can receive.
enum Message {
    Input(String),
    JournalResponse(Vec<String>),
}

impl Component for Monitor {
    // What this component needs from the world.
    type Io = Io;

    // What this component needs as static config, including program connections.
    type Connections = Connector<JournalMessage>;

    type Configuration = ();

    // The only thing this component can mutate directly.
    type Model = Model;

    // What Self::update processes.
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
                Command::request(&cx.to, move |reply| JournalMessage::Recent {
                    count: if s == "5" { 5 } else { 10 },
                    reply,
                })
                .producing(|response| match response {
                    RequestOutcome::Replied(r) => Message::JournalResponse(r.unwrap()),
                    // This would handle error or cancellation.
                    _ => todo!(),
                })
            }
            Message::JournalResponse(r) => samara::println!(&cx.io.output, "Got response: {:?}", r),
        }
    }

    fn subscriptions(cx: &Context<Self>, _model: &Self::Model) -> Subscriptions<Self::Message> {
        Subscriptions::one(Subscription::source_with(
            &cx.io.input,
            StdinLines::new(),
            |event| match event {
                SourceEvent::Item(s) => Message::Input(s),
                // This would handle EOF or errors.
                _ => todo!(),
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

    // Run it until Ctrl-C, then let every Component finish.
    runtime.run_until(tokio::signal::ctrl_c()).await?;

    Ok(())
}
