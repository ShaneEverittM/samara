use anyhow::Result;
use samara::prelude::*;
use std::time::Duration;
use url::Url;
use urlchecker::*;

fn fixture() -> Result<(ControlledRuntime, ComponentRef<Checker>)> {
    let (program, (checker,), _) = program()?;
    let runtime = ControlledRuntime::builder(program)
        .control_effect::<PrintStdout>()
        .control_effect::<PrintStderr>()
        .control_effect::<HttpRequest>()
        .control_source::<StdinLines>()
        .build()?;

    Ok((runtime, checker))
}

fn url(url: &str) -> Result<Message> {
    let message = Message::Input(InputEvent::Command(UrlCommand::Url(Url::parse(url)?)));
    Ok(message)
}

#[tokio::test]
async fn does_nothing_on_eof() -> Result<()> {
    let (mut runtime, checker) = fixture()?;

    let status = runtime.state(&checker)?;
    assert!(matches!(status, Status::Idle { .. }));
    runtime.send(&checker, Message::Input(InputEvent::Eof))?;
    runtime.run_until_idle()?;
    assert!(runtime.next_effect::<HttpRequest>().is_err());

    Ok(())
}

#[tokio::test]
async fn emits_request_after_fixed_delay() -> Result<()> {
    let (mut runtime, checker) = fixture()?;

    runtime.send(&checker, url("https://google.com")?)?;

    runtime.advance(Checker::DELAY - Duration::from_millis(1))?;
    assert!(runtime.next_effect::<HttpRequest>().is_err());

    runtime.advance(Duration::from_millis(1))?;
    let request = runtime.next_effect::<HttpRequest>()?;
    assert_eq!(request.intent.url(), "https://google.com/");
    Ok(())
}

#[tokio::test]
async fn second_request_replaces_the_first_during_debounce() -> Result<()> {
    let (mut runtime, checker) = fixture()?;

    // If half-way through the debounce period, a second check
    // is requested, the first request is not initiated at all.
    runtime.send(&checker, url("https://google.com")?)?;
    runtime.advance(Checker::DELAY / 2)?;
    runtime.send(&checker, url("https://wikipedia.com")?)?;
    runtime.advance(Checker::DELAY / 2)?;
    assert!(runtime.next_effect::<HttpRequest>().is_err());

    // However, 500ms from the second request, we should
    // see a request initiated.
    runtime.advance(Checker::DELAY / 2)?;
    let request = runtime.next_effect::<HttpRequest>()?;
    assert_eq!(request.intent.url(), "https://wikipedia.com/");

    Ok(())
}

#[tokio::test]
async fn second_request_replaces_the_first_during_in_flight_period() -> Result<()> {
    let (mut runtime, checker) = fixture()?;

    // Start A and keep its result pending while B is selected and started.
    runtime.send(&checker, url("https://google.com")?)?;
    runtime.advance(Checker::DELAY)?;
    let first = runtime.next_effect::<HttpRequest>()?;
    assert_eq!(first.intent.url(), "https://google.com/");
    assert!(runtime.next_effect::<HttpRequest>().is_err());

    runtime.send(&checker, url("https://wikipedia.com")?)?;
    runtime.advance(Checker::DELAY)?;
    let second = runtime.next_effect::<HttpRequest>()?;
    assert_eq!(second.intent.url(), "https://wikipedia.com/");
    assert!(runtime.next_effect::<HttpRequest>().is_err());

    // Before either response, each selection prints its debounce and start messages.
    for expected in [
        "Waiting 500ms before checking...\n",
        "Checking https://google.com/\n",
        "Waiting 500ms before checking...\n",
        "Checking https://wikipedia.com/\n",
    ] {
        let output = runtime.next_effect::<PrintStdout>()?;
        assert_eq!(output.intent.as_str(), expected);
        runtime.complete(output, EffectOutcome::Succeeded(()))?;
    }
    runtime.run_until_idle()?;
    assert!(runtime.next_effect::<PrintStdout>().is_err());
    assert!(runtime.next_effect::<PrintStderr>().is_err());

    // B finishes first: its result becomes visible and is printed once.
    runtime.complete(
        second,
        EffectOutcome::Succeeded(HttpResponse::new(
            StatusCode::OK,
            Default::default(),
            Default::default(),
            "",
        )),
    )?;
    runtime.run_until_idle()?;
    let second_revision = match runtime.state(&checker)? {
        Status::Finished {
            url,
            revision,
            result: CheckResult::Response(status),
        } => {
            assert_eq!(url.as_ref(), "https://wikipedia.com/");
            assert_eq!(*status, StatusCode::OK);
            *revision
        }
        _ => panic!("B should have a completed HTTP result"),
    };
    let output = runtime.next_effect::<PrintStdout>()?;
    assert_eq!(output.intent.as_str(), "https://wikipedia.com/: 200 OK\n");
    runtime.complete(output, EffectOutcome::Succeeded(()))?;
    runtime.run_until_idle()?;
    assert!(runtime.next_effect::<PrintStdout>().is_err());
    assert!(runtime.next_effect::<PrintStderr>().is_err());

    // A finishes late with a different result: B's state and output must survive.
    runtime.complete(
        first,
        EffectOutcome::Succeeded(HttpResponse::new(
            StatusCode::NOT_FOUND,
            Default::default(),
            Default::default(),
            "",
        )),
    )?;
    let report = runtime.run_until_idle()?;
    assert_eq!(report.transitions, 1);
    assert!(matches!(
        runtime.state(&checker)?,
        Status::Finished {
            url,
            revision,
            result: CheckResult::Response(status),
        } if url.as_ref() == "https://wikipedia.com/"
            && *revision == second_revision
            && *status == StatusCode::OK
    ));
    assert!(runtime.next_effect::<PrintStdout>().is_err());
    assert!(runtime.next_effect::<PrintStderr>().is_err());
    assert!(runtime.next_effect::<HttpRequest>().is_err());

    assert!(runtime.cancel()?.is_clean());

    Ok(())
}
