use samara::{Runtime, Shutdown};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (program, io) = urlchecker::program()?;

    let runtime = Runtime::builder(program)
        .supply_default(&io.stdin)
        .supply_default(&io.stdout)
        .supply_default(&io.stderr)
        .supply_default(&io.http)
        .build()?;

    let mut task = runtime.spawn();
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    tokio::select! {
        result = task.run_forever() => {
            result?;
            return Ok(());
        }
        signal = &mut ctrl_c => signal?,
    }

    task.shutdown(Shutdown::Cancel).await?;

    Ok(())
}
