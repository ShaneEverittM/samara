use pulse::journal::Journal;
use samara::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut builder = Program::builder();

    let journal = builder.install::<Journal>(());
    // Monitor will be assembled here against `journal.exports()`.

    let program = builder.build()?;

    let runtime = Runtime::builder(program)
        .with_assembly(journal)
        .and_its_vendored_drivers()
        .build()?;

    let task = runtime.spawn();
    tokio::signal::ctrl_c().await?;
    task.shutdown(Shutdown::Drain).await?;

    Ok(())
}
