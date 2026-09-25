use samara::{LiveRuntime, Shutdown};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let program = urlchecker::program()?;

    let runtime = LiveRuntime::builder(program)
        .bind_http()
        .bind_stdio()
        .bind_stdin()
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
