use samara::{
    Capabilities, Command, Component, Connector, Effect, FileDriver, Init, PrintStdout, Program,
    RequestOutcome, Runtime, Source, SourceEvent, StdinDriver, StdinLines, StdoutDriver,
    Subscription, Subscriptions, SystemClockDriver,
};

// This is a "library assembly", that is a subsection of a Program (discussed later)
// designed to be embedded in another program.
use pulse::journal::{Journal, JournalMessage};

// This is the component of our application.
struct Monitor;

// What it can reach in the world, by name.
#[derive(Clone, Capabilities)]
struct MonitorIo {
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
    type Io = MonitorIo;

    // What this component needs as static config, including program connections.
    type Config = Connector<JournalMessage>;

    // The only thing this component can mutate directly.
    type Model = Model;

    // What Self::update processes.
    type Message = Message;

    fn init(_: &Self::Config, _: &Self::Io) -> Init<Self::Model, Self::Message> {
        Init::new(Model)
    }

    fn update(
        journal: &Self::Config,
        io: &MonitorIo,
        _: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        match message {
            Message::Input(s) => Command::request(journal, move |reply| JournalMessage::Recent {
                count: if s == "5" { 5 } else { 10 },
                reply,
            })
            .producing(|response| match response {
                RequestOutcome::Replied(r) => Message::JournalResponse(r.unwrap()),
                // This would handle error or cancellation.
                _ => todo!(),
            }),
            Message::JournalResponse(r) => samara::println!(&io.output, "Got response: {:?}", r),
        }
    }

    fn subscriptions(
        _: &Self::Config,
        io: &MonitorIo,
        _model: &Self::Model,
    ) -> Subscriptions<Self::Message> {
        Subscriptions::one(Subscription::source_with(
            &io.input,
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
    // A program is dormant, it declares how things are internally connected.
    let mut builder = Program::builder();

    let journal = builder.assembly::<Journal>(());

    let monitor = builder.component::<Monitor>(journal.connector());

    // Assemble the program, this checks things that _can't_ be checked at compile time.
    let program = builder.build()?;

    // Create a runtime for our program...
    let runtime = Runtime::builder(program)
        // ...and say what each capability is made of.
        .supply(&journal.io.file, FileDriver::new("./data/pulse.journal"))
        .supply(&journal.io.clock, SystemClockDriver)
        .supply(&monitor.io.input, StdinDriver)
        .supply(&monitor.io.output, StdoutDriver::new())
        .build()?;

    // Run it until Ctrl-C, then let every Component finish.
    runtime.run_until(tokio::signal::ctrl_c()).await?;

    Ok(())
}
