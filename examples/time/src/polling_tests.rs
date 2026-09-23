//! Polling lifecycle evidence for docs/notes/first-real-application.md.

use super::*;

const POLL_INTERVAL: Duration = Duration::from_secs(1);

fn timestamp(seconds: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(seconds, 0).unwrap()
}

fn http_response(status: u16, body: String) -> HttpResponse {
    HttpResponse::new(
        status.try_into().unwrap(),
        Default::default(),
        Default::default(),
        body,
    )
}

fn time_response(time: DateTime<Utc>) -> HttpResponse {
    http_response(
        200,
        format!(r#"{{"data":{{"iso":"{}"}}}}"#, time.to_rfc3339()),
    )
}

fn failed_http_outcomes() -> Vec<EffectOutcome<HttpResponse, HttpError>> {
    vec![
        EffectOutcome::Failed(HttpError::new(HttpErrorKind::Transport, "offline")),
        EffectOutcome::Succeeded(http_response(503, "unavailable".to_owned())),
        EffectOutcome::Succeeded(http_response(200, "invalid JSON".to_owned())),
        EffectOutcome::Cancelled(CancelReason::Deadline),
    ]
}

fn http_fixture() -> HttpTimeServer {
    let mut builder = Program::builder();
    HttpTimeServer {
        http: builder.effect::<HttpRequest>(),
        stderr: builder.effect::<PrintStderr>(),
    }
}

struct TimeReader(Port<TimeServerProtocol>);

enum ReadMessage {
    Read,
    Received(RequestOutcome<Option<DateTime<Utc>>>),
}

impl From<RequestOutcome<Option<DateTime<Utc>>>> for ReadMessage {
    fn from(outcome: RequestOutcome<Option<DateTime<Utc>>>) -> Self {
        Self::Received(outcome)
    }
}

impl Component for TimeReader {
    type Model = Vec<Option<DateTime<Utc>>>;
    type Message = ReadMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::default()
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            ReadMessage::Read => Command::request(self.0.clone(), GetCurrentTime),
            ReadMessage::Received(RequestOutcome::Replied(time)) => {
                model.push(time);
                Command::none()
            }
            ReadMessage::Received(outcome) => panic!("unexpected cached-read outcome: {outcome:?}"),
        }
    }
}

fn controlled_http_fixture() -> (
    ControlledRuntime,
    ComponentRef<HttpTimeServer>,
    ComponentRef<TimeReader>,
) {
    let mut builder = Program::builder();
    let http = builder.effect::<HttpRequest>();
    let stderr = builder.effect::<PrintStderr>();
    let port = builder.port(PortId::new(TIME_SERVER_PORT));
    let server = builder.component(
        ComponentId::new("time-server"),
        HttpTimeServer { http, stderr },
    );
    builder.bind_port(&port, &server);
    let reader = builder.component(ComponentId::new("reader"), TimeReader(port));
    let runtime = ControlledRuntime::builder(builder.build().unwrap())
        .control_effect::<HttpRequest>()
        .control_effect::<PrintStderr>()
        .build()
        .unwrap();
    (runtime, server, reader)
}

#[test]
fn http_idle_ignores_unsolicited_poll_completion() {
    for cached in [None, Some(timestamp(100))] {
        let mut outcomes = failed_http_outcomes();
        outcomes.push(EffectOutcome::Succeeded(time_response(timestamp(200))));
        for outcome in outcomes {
            let server = http_fixture();
            let mut model = HttpTimeModel::Idle {
                current_time: cached,
            };
            // Decode through the normal mapper, but deliver without issuing a poll.
            let message = HttpRequest::get("https://example.test/time")
                .on_response()
                .require_success()
                .json::<TimeResponse>()
                .into_command(&server.http)
                .map_effect_outcome::<HttpRequest>(outcome)
                .ok()
                .unwrap();
            let command = server.update(&mut model, message);
            assert!(command.is_none(), "Idle has no poll to complete");
            assert_eq!(
                model,
                HttpTimeModel::Idle {
                    current_time: cached
                }
            );
        }
    }
}

#[test]
fn http_poll_skips_busy_ticks() {
    let server = http_fixture();
    let mut model = server.init().model;
    assert_eq!(model, HttpTimeModel::Idle { current_time: None });
    let first = server.update(&mut model, HttpTimeMessage::Tick);
    assert_eq!(first.effect_intents::<HttpRequest>().len(), 1);
    for _ in 0..3 {
        let busy = server.update(&mut model, HttpTimeMessage::Tick);
        assert!(
            busy.effect_intents::<HttpRequest>().is_empty(),
            "busy ticks must skip HTTP work"
        );
    }
    assert_eq!(model, HttpTimeModel::Polling { current_time: None });
}

#[test]
fn http_poll_terminal_outcomes_update_state_and_allow_next_poll() {
    let mut outcomes = failed_http_outcomes();
    outcomes.push(EffectOutcome::Succeeded(time_response(timestamp(200))));
    let success_index = outcomes.len() - 1;
    for (index, outcome) in outcomes.into_iter().enumerate() {
        let server = http_fixture();
        let mut model = HttpTimeModel::Idle {
            current_time: Some(timestamp(100)),
        };
        let first = server.update(&mut model, HttpTimeMessage::Tick);
        assert_eq!(
            model,
            HttpTimeModel::Polling {
                current_time: Some(timestamp(100))
            }
        );
        let invocation = first
            .into_declarations()
            .into_iter()
            .find_map(|command| command.into_effect::<HttpRequest>().ok())
            .unwrap();
        let terminal = invocation.map_outcome(outcome);
        let follow_up = server.update(&mut model, terminal);
        let succeeded = index == success_index;
        assert_eq!(
            model,
            HttpTimeModel::Idle {
                current_time: Some(timestamp(if succeeded { 200 } else { 100 }))
            }
        );
        assert_eq!(
            follow_up.effect_intents::<PrintStderr>().len(),
            usize::from(!succeeded)
        );
        assert!(
            follow_up.effect_intents::<HttpRequest>().is_empty(),
            "wait for the next tick"
        );
        let next = server.update(&mut model, HttpTimeMessage::Tick);
        assert_eq!(next.effect_intents::<HttpRequest>().len(), 1);
        assert!(
            server
                .update(&mut model, HttpTimeMessage::Tick)
                .effect_intents::<HttpRequest>()
                .is_empty()
        );
    }
}

#[test]
fn controlled_slow_poll_skips_work_and_serves_cached_reads() {
    let (mut runtime, server, reader) = controlled_http_fixture();
    runtime.advance(POLL_INTERVAL / 2).unwrap();
    assert!(
        runtime.next_effect::<HttpRequest>().is_err(),
        "initial delay is preserved"
    );
    runtime.advance(POLL_INTERVAL / 2).unwrap();
    let first = runtime.next_effect::<HttpRequest>().unwrap();
    runtime.advance(POLL_INTERVAL * 4).unwrap();
    assert!(
        runtime.next_effect::<HttpRequest>().is_err(),
        "slow HTTP must not accumulate more calls"
    );
    assert_eq!(
        runtime.pending_work(),
        PendingWork {
            pending_now: 0,
            pending_later: 2
        }
    );
    runtime.send(&reader, ReadMessage::Read).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(runtime.state(&reader).unwrap(), &[None]);

    runtime
        .complete(
            first,
            EffectOutcome::Succeeded(time_response(timestamp(200))),
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime.next_effect::<HttpRequest>().is_err(),
        "no catch-up call on completion"
    );
    runtime.advance(POLL_INTERVAL).unwrap();
    let second = runtime.next_effect::<HttpRequest>().unwrap();
    runtime.send(&reader, ReadMessage::Read).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime.state(&reader).unwrap(),
        &[None, Some(timestamp(200))]
    );
    runtime.advance(POLL_INTERVAL * 3).unwrap();
    assert!(runtime.next_effect::<HttpRequest>().is_err());
    // The provider may legitimately return an earlier clock value. Freshness
    // means no overlapping older invocation, not max(timestamp).
    runtime
        .complete(
            second,
            EffectOutcome::Succeeded(time_response(timestamp(100))),
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime.state(&server).unwrap(),
        &HttpTimeModel::Idle {
            current_time: Some(timestamp(100))
        }
    );
    runtime.send(&reader, ReadMessage::Read).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime.state(&reader).unwrap(),
        &[None, Some(timestamp(200)), Some(timestamp(100))]
    );
    assert!(runtime.cancel().unwrap().is_clean());
}

#[test]
fn controlled_failed_polls_resume_on_next_tick_without_resetting_cadence() {
    for failure in failed_http_outcomes() {
        let (mut runtime, server, _) = controlled_http_fixture();
        runtime.advance(POLL_INTERVAL).unwrap();
        let first = runtime.next_effect::<HttpRequest>().unwrap();
        runtime
            .complete(
                first,
                EffectOutcome::Succeeded(time_response(timestamp(100))),
            )
            .unwrap();
        runtime.run_until_idle().unwrap();
        runtime.advance(POLL_INTERVAL).unwrap();
        let second = runtime.next_effect::<HttpRequest>().unwrap();
        runtime.advance(POLL_INTERVAL / 2).unwrap();
        runtime.complete(second, failure).unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(
            runtime.state(&server).unwrap().current_time(),
            Some(timestamp(100))
        );
        let error = runtime.next_effect::<PrintStderr>().unwrap();
        runtime
            .complete(error, EffectOutcome::Succeeded(()))
            .unwrap();
        runtime.run_until_idle().unwrap();
        assert!(runtime.next_effect::<HttpRequest>().is_err());
        // Completion at 2.5s must not move the next existing tick from 3s.
        runtime.advance(POLL_INTERVAL / 2).unwrap();
        let _third = runtime.next_effect::<HttpRequest>().unwrap();
        assert!(runtime.next_effect::<HttpRequest>().is_err());
        runtime.advance(POLL_INTERVAL * 2).unwrap();
        assert!(runtime.next_effect::<HttpRequest>().is_err());
        assert!(runtime.cancel().unwrap().is_clean());
    }
}

#[test]
fn controlled_tick_completion_orders_both_preserve_single_poll() {
    for completion_first in [false, true] {
        let run = || {
            let (mut runtime, server, _) = controlled_http_fixture();
            runtime.advance(POLL_INTERVAL).unwrap();
            let first = runtime.next_effect::<HttpRequest>().unwrap();
            // Queue a tick and terminal outcome at the same logical instant
            // in each order; neither order is a promise about live execution.
            if !completion_first {
                runtime.send(&server, HttpTimeMessage::Tick).unwrap();
            }
            runtime
                .complete(
                    first,
                    EffectOutcome::Succeeded(time_response(timestamp(100))),
                )
                .unwrap();
            if completion_first {
                runtime.send(&server, HttpTimeMessage::Tick).unwrap();
            }
            runtime.run_until_idle().unwrap();
            let next = runtime.next_effect::<HttpRequest>();
            assert_eq!(next.is_ok(), completion_first);
            assert!(runtime.next_effect::<HttpRequest>().is_err());
            assert_eq!(
                runtime.state(&server).unwrap().current_time(),
                Some(timestamp(100))
            );
            if let Ok(next) = next {
                runtime
                    .complete(
                        next,
                        EffectOutcome::Succeeded(time_response(timestamp(200))),
                    )
                    .unwrap();
                runtime.run_until_idle().unwrap();
                assert_eq!(
                    runtime.state(&server).unwrap().current_time(),
                    Some(timestamp(200))
                );
            }
            let trace = runtime.trace().to_vec();
            assert!(runtime.cancel().unwrap().is_clean());
            trace
        };
        assert_eq!(run(), run());
    }
}
