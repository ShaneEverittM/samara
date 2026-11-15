use samara::{
    Capabilities, Command, Component, Connector, Effect, FileDriver, Init, PrintStdout,
    ProgramBuilder, RequestOutcome, Runtime, Shutdown, Source, SourceEvent, StdinDriver,
    StdinLines, StdoutDriver, Subscription, Subscriptions, SystemClockDriver,
};

// This is a "library assembly", that is a subsection of a Program (discussed later)
// designed to be embedded in another program.
use pulse::journal::{Journal, JournalMessage};

// This is the component of our application>
struct Monitor;

// Stateless for now.
struct Model;

// Messages our model can receive.
enum Message {
    Input(String),
    JournalResponse(Vec<String>),
}

impl Component for Monitor {
    // What this component needs from the world.
    type Requires = (Source<StdinLines>, Effect<PrintStdout>);

    // What this component needs as static config, including program connections.
    type Config = Connector<JournalMessage>;

    // The only thing this component can mutate directly.
    type Model = Model;

    // What Self::update processes.
    type Message = Message;

    fn init(
        _: &Self::Config,
        _: &Capabilities<Self::Requires>,
    ) -> Init<Self::Model, Self::Message> {
        Init::new(Model)
    }

    fn update(
        journal: &Self::Config,
        (_, stdout): &Capabilities<Self::Requires>,
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
            Message::JournalResponse(r) => samara::println!(&stdout, "Got response: {:?}", r),
        }
    }

    fn subscriptions(
        _: &Self::Config,
        (stdin, ..): &Capabilities<Self::Requires>,
        _model: &Self::Model,
    ) -> Subscriptions<Self::Message> {
        Subscriptions::one(Subscription::source_with(
            stdin,
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
    let mut builder = ProgramBuilder::new();

    let journal = builder.install::<Journal>(()).finish()?;

    let monitor = builder
        .component::<Monitor>(journal.connector().clone())
        .finish()?;

    // Assemble the program, this checks things that _can't_ be checked at compile time.
    let program = builder.build()?;

    // Create a runtime for our program...
    let runtime = Runtime::builder(program)
        .with(journal.capabilities())
        .supplied_by((FileDriver::new("./data/pulse.journal"), SystemClockDriver))
        .with(monitor.capabilities())
        .supplied_by((StdinDriver, StdoutDriver::new()))
        .build()?;

    // Run it!
    let task = runtime.spawn();
    tokio::signal::ctrl_c().await?;
    task.shutdown(Shutdown::Drain).await?;

    Ok(())
}
