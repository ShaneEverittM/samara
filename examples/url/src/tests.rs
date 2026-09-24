use super::*;
use anyhow::Result;

fn fixture() -> Result<(ControlledRuntime, ComponentRef<Checker>)> {
    let (program, checker, _) = assemble()?;
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

fn input(
    runtime: &mut ControlledRuntime,
    checker: &ComponentRef<Checker>,
    line: &str,
) -> Result<()> {
    runtime.emit_source::<_, StdinLines>(
        checker,
        &SubscriptionId::new("commands"),
        SourceEvent::Item(line.into()),
    )?;
    runtime.run_until_idle()?;
    Ok(())
}

// Inspect and finish exactly these writes, including asserting silence with &[].
fn expect_stdout(runtime: &mut ControlledRuntime, expected: &[&str]) -> Result<()> {
    for text in expected {
        let output = runtime.next_effect::<PrintStdout>()?;
        assert_eq!(output.intent.as_str(), *text);
        runtime.complete(output, EffectOutcome::Succeeded(()))?;
    }
    assert!(runtime.next_effect::<PrintStdout>().is_err());
    assert!(runtime.next_effect::<PrintStderr>().is_err());
    Ok(())
}

fn response(status: StatusCode) -> EffectOutcome<HttpResponse, HttpError> {
    EffectOutcome::Succeeded(HttpResponse::new(
        status,
        Default::default(),
        Default::default(),
        "",
    ))
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

#[test]
fn clear_ignores_old_timers_even_after_reselecting_the_same_url() -> Result<()> {
    for reselect in [false, true] {
        let (mut runtime, checker) = fixture()?;
        input(&mut runtime, &checker, "url https://google.com")?;
        expect_stdout(&mut runtime, &["Waiting 500ms before checking...\n"])?;
        runtime.advance(Checker::DELAY / 2)?;

        // Repeated clear must not reset the revision either.
        input(&mut runtime, &checker, "clear")?;
        input(&mut runtime, &checker, "clear")?;
        expect_stdout(&mut runtime, &["Cleared\n", "Cleared\n"])?;
        if reselect {
            input(&mut runtime, &checker, "url https://google.com")?;
            expect_stdout(&mut runtime, &["Waiting 500ms before checking...\n"])?;
        }

        // The original deadline arrives; it must neither start HTTP nor log Checking.
        let report = runtime.advance(Checker::DELAY / 2)?;
        assert_eq!(report.transitions, 1);
        assert!(runtime.next_effect::<HttpRequest>().is_err());
        expect_stdout(&mut runtime, &[])?;
        if reselect {
            assert!(
                matches!(runtime.state(&checker)?, Status::Pending { url, .. }
                if url.as_ref() == "https://google.com/")
            );
            runtime.advance(Checker::DELAY / 2)?;
            let request = runtime.next_effect::<HttpRequest>()?;
            assert_eq!(request.intent.url(), "https://google.com/");
            assert!(runtime.next_effect::<HttpRequest>().is_err());
            expect_stdout(&mut runtime, &["Checking https://google.com/\n"])?;
        } else {
            assert!(matches!(runtime.state(&checker)?, Status::Idle { .. }));
        }
        assert!(runtime.cancel()?.is_clean());
    }
    Ok(())
}

#[test]
fn clear_ignores_old_results_even_after_reselecting_the_same_url() -> Result<()> {
    for reselect in [false, true] {
        let (mut runtime, checker) = fixture()?;
        input(&mut runtime, &checker, "url https://google.com")?;
        runtime.advance(Checker::DELAY)?;
        let old = runtime.next_effect::<HttpRequest>()?;
        expect_stdout(
            &mut runtime,
            &[
                "Waiting 500ms before checking...\n",
                "Checking https://google.com/\n",
            ],
        )?;

        input(&mut runtime, &checker, "clear")?;
        input(&mut runtime, &checker, "clear")?;
        expect_stdout(&mut runtime, &["Cleared\n", "Cleared\n"])?;
        if reselect {
            input(&mut runtime, &checker, "url https://google.com")?;
            expect_stdout(&mut runtime, &["Waiting 500ms before checking...\n"])?;
        }

        runtime.complete(old, response(StatusCode::NOT_FOUND))?;
        assert_eq!(runtime.run_until_idle()?.transitions, 1);
        expect_stdout(&mut runtime, &[])?;
        assert!(runtime.next_effect::<HttpRequest>().is_err());
        if reselect {
            assert!(
                matches!(runtime.state(&checker)?, Status::Pending { url, .. }
                if url.as_ref() == "https://google.com/")
            );
            runtime.advance(Checker::DELAY)?;
            let new = runtime.next_effect::<HttpRequest>()?;
            expect_stdout(&mut runtime, &["Checking https://google.com/\n"])?;
            runtime.complete(new, response(StatusCode::OK))?;
            runtime.run_until_idle()?;
            assert!(matches!(
                runtime.state(&checker)?,
                Status::Finished {
                    result: CheckResult::Response(StatusCode::OK),
                    ..
                }
            ));
            expect_stdout(&mut runtime, &["https://google.com/: 200 OK\n"])?;
        } else {
            assert!(matches!(runtime.state(&checker)?, Status::Idle { .. }));
        }
        assert!(runtime.cancel()?.is_clean());
    }
    Ok(())
}

#[test]
fn reselecting_the_same_url_ignores_the_old_result_during_debounce() -> Result<()> {
    let (mut runtime, checker) = fixture()?;
    input(&mut runtime, &checker, "url https://google.com")?;
    runtime.advance(Checker::DELAY)?;
    let old = runtime.next_effect::<HttpRequest>()?;
    input(&mut runtime, &checker, "url https://google.com")?;
    expect_stdout(
        &mut runtime,
        &[
            "Waiting 500ms before checking...\n",
            "Checking https://google.com/\n",
            "Waiting 500ms before checking...\n",
        ],
    )?;

    runtime.complete(old, response(StatusCode::NOT_FOUND))?;
    assert_eq!(runtime.run_until_idle()?.transitions, 1);
    assert!(
        matches!(runtime.state(&checker)?, Status::Pending { url, .. }
        if url.as_ref() == "https://google.com/")
    );
    expect_stdout(&mut runtime, &[])?;
    runtime.advance(Checker::DELAY)?;
    assert_eq!(
        runtime.next_effect::<HttpRequest>()?.intent.url(),
        "https://google.com/"
    );
    expect_stdout(&mut runtime, &["Checking https://google.com/\n"])?;
    assert!(runtime.cancel()?.is_clean());
    Ok(())
}

#[test]
fn status_and_clear_through_stdin_have_repeatable_traces() -> Result<()> {
    let mut traces = Vec::new();
    for _ in 0..2 {
        let (mut runtime, checker) = fixture()?;
        input(&mut runtime, &checker, "status")?;
        expect_stdout(
            &mut runtime,
            &["URL: none\nState: idle\nLast result: none\n"],
        )?;
        input(&mut runtime, &checker, "url https://google.com")?;
        input(&mut runtime, &checker, "status")?;
        expect_stdout(
            &mut runtime,
            &[
                "Waiting 500ms before checking...\n",
                "URL: https://google.com/\nState: waiting\nLast result: none\n",
            ],
        )?;
        runtime.advance(Checker::DELAY)?;
        let request = runtime.next_effect::<HttpRequest>()?;
        input(&mut runtime, &checker, "status")?;
        expect_stdout(
            &mut runtime,
            &[
                "Checking https://google.com/\n",
                "URL: https://google.com/\nState: checking\nLast result: none\n",
            ],
        )?;
        runtime.complete(request, response(StatusCode::OK))?;
        runtime.run_until_idle()?;
        input(&mut runtime, &checker, "status")?;
        expect_stdout(
            &mut runtime,
            &[
                "https://google.com/: 200 OK\n",
                "URL: https://google.com/\nState: idle\nLast result: 200 OK\n",
            ],
        )?;

        // Clearing a finished check forgets both the URL and the result.
        input(&mut runtime, &checker, "clear")?;
        input(&mut runtime, &checker, "status")?;
        assert!(matches!(runtime.state(&checker)?, Status::Idle { .. }));
        expect_stdout(
            &mut runtime,
            &["Cleared\n", "URL: none\nState: idle\nLast result: none\n"],
        )?;
        assert!(runtime.next_effect::<HttpRequest>().is_err());
        traces.push(runtime.trace().to_vec());
        assert!(runtime.cancel()?.is_clean());
    }
    assert_eq!(traces[0], traces[1]);
    Ok(())
}

#[test]
fn http_error_status_transport_failure_and_cancellation_remain_distinct() -> Result<()> {
    for (outcome, expected) in [
        (response(StatusCode::NOT_FOUND), "404 Not Found"),
        (
            EffectOutcome::Failed(HttpError::new(HttpErrorKind::Transport, "connection reset")),
            "Check failed: HTTP Transport error: connection reset",
        ),
        (EffectOutcome::Cancelled(CancelReason::Deadline), "Canceled"),
    ] {
        let (mut runtime, checker) = fixture()?;
        input(&mut runtime, &checker, "url https://google.com")?;
        runtime.advance(Checker::DELAY)?;
        let request = runtime.next_effect::<HttpRequest>()?;
        expect_stdout(
            &mut runtime,
            &[
                "Waiting 500ms before checking...\n",
                "Checking https://google.com/\n",
            ],
        )?;
        runtime.complete(request, outcome)?;
        runtime.run_until_idle()?;
        match runtime.state(&checker)? {
            Status::Finished { url, result, .. } => {
                assert_eq!(url.as_ref(), "https://google.com/");
                assert_eq!(result.to_string(), expected);
            }
            _ => panic!("check should finish with {expected}"),
        }
        input(&mut runtime, &checker, "status")?;
        expect_stdout(
            &mut runtime,
            &[
                &format!("https://google.com/: {expected}\n"),
                &format!("URL: https://google.com/\nState: idle\nLast result: {expected}\n"),
            ],
        )?;
        assert!(runtime.next_effect::<HttpRequest>().is_err());
        assert!(runtime.cancel()?.is_clean());
    }
    Ok(())
}

#[test]
fn malformed_stdin_leaves_the_pending_selection_unchanged() -> Result<()> {
    let (mut runtime, checker) = fixture()?;
    input(&mut runtime, &checker, "url https://google.com")?;
    expect_stdout(&mut runtime, &["Waiting 500ms before checking...\n"])?;
    let revision = match runtime.state(&checker)? {
        Status::Pending { revision, .. } => *revision,
        _ => panic!("selection should be waiting"),
    };
    let usage = "usage: url <address> | status | clear\n";
    for (line, expected) in [
        ("", usage),
        ("wat", usage),
        ("url", usage),
        ("url not-a-url", "Invalid URL: not-a-url\n"),
        ("status extra", usage),
        ("clear extra", usage),
    ] {
        input(&mut runtime, &checker, line)?;
        assert!(
            matches!(runtime.state(&checker)?, Status::Pending { url, revision: current }
            if url.as_ref() == "https://google.com/" && *current == revision)
        );
        let error = runtime.next_effect::<PrintStderr>()?;
        assert_eq!(error.intent.as_str(), expected);
        runtime.complete(error, EffectOutcome::Succeeded(()))?;
        expect_stdout(&mut runtime, &[])?;
        assert!(runtime.next_effect::<HttpRequest>().is_err());
    }
    runtime.advance(Checker::DELAY)?;
    assert_eq!(
        runtime.next_effect::<HttpRequest>()?.intent.url(),
        "https://google.com/"
    );
    expect_stdout(&mut runtime, &["Checking https://google.com/\n"])?;
    assert!(runtime.cancel()?.is_clean());
    Ok(())
}
