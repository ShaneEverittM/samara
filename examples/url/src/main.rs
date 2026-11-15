use samara::{Runtime, Shutdown};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (program, io) = urlchecker::program()?;

    let runtime = Runtime::builder(program)
        .supply(&io.stdin, samara::StdinDriver)
        .supply(&io.stdout, samara::StdoutDriver::new())
        .supply(&io.stderr, samara::StderrDriver::new())
        .supply(&io.http, samara::HttpDriver::new())
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
