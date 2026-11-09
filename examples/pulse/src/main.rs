use samara::{
    Assembly, Capabilities, Command, Component, ComponentId, Connector, Effect, Init, PrintStdout,
    Program, RequestOutcome, Runtime, Shutdown, Source, SourceEvent, StdinDriver, StdinLines,
    StdoutDriver, Subscription, SubscriptionId, Subscriptions, VendorsDrivers,
};

// This is a "library assembly", that is a subsection of a Program (discussed later)
// designed to be embedded in another program.
use pulse::journal::{Journal, JournalMessage};

// This is the component of our application>
struct Monitor;

struct Config {
    journal: Connector<JournalMessage>,
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
    type Requires = (Source<StdinLines>, Effect<PrintStdout>);

    // What this component needs as static config, including program connections.
    type Config = Config;

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
        Config { journal }: &Self::Config,
        (_, stdout): &Capabilities<Self::Requires>,
        _: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        match message {
            Message::Input(s) => Command::request_to(
                journal,
                move |reply| JournalMessage::Recent {
                    count: if s == "5" { 5 } else { 10 },
                    reply_with: reply,
                },
                |response| match response {
                    RequestOutcome::Replied(r) => Message::JournalResponse(r),
                    // This would handle error or cancellation.
                    _ => todo!(),
                },
            ),
            Message::JournalResponse(r) => samara::println!(&stdout, "Got response: {:?}", r),
        }
    }

    fn subscriptions(
        _: &Self::Config,
        (stdin, ..): &Capabilities<Self::Requires>,
        _model: &Self::Model,
    ) -> Subscriptions<Self::Message> {
        Subscriptions::one(Subscription::source_with(
            &stdin,
            SubscriptionId::new("input"),
            StdinLines::new(),
            |event| match event {
                SourceEvent::Item(s) => Message::Input(s),
                // This would handle EOF or errors.
                _ => todo!(),
            },
        ))
    }
}

// Declare you applications requirements, it can be manual,
// or a composition of Components and Assemblies requirements
// as shown here.
type AppRequirements = (
    <Journal as Assembly>::Requires,
    <Monitor as Component>::Requires,
);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // A program is dormant, it declares how things are internally connected.
    let (mut builder, (journal_caps, monitor_caps)) = Program::builder::<AppRequirements>();

    // Re-use the library assembly...
    let journal = builder
        .install::<Journal>(())
        // ...failing to compile if you don't give it its declared capabilities.
        .with_capabilities(journal_caps)
        .finish()?;

    // Install our application component...
    let _monitor = builder
        .component::<Monitor>(
            ComponentId::new("monitor"),
            Config {
                journal: journal.connector().clone(),
            },
        )
        // ...that also requires its capabilities.
        .with_capabilities(monitor_caps)
        .finish()?;

    // Assemble the program, this checks things that _can't_ be checked at compile time.
    let program = builder.build()?;

    // Create a runtime for our program...
    let runtime = Runtime::builder(program)
        // ...and install the drivers that actually supply the behavior to the capabilities.
        .with_drivers((Journal::drivers(), (StdinDriver, StdoutDriver::new())))
        .build()?;

    // Run it!
    let task = runtime.spawn();
    tokio::signal::ctrl_c().await?;
    task.shutdown(Shutdown::Drain).await?;

    Ok(())
}
