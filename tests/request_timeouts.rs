//! ADR-0011: one request outcome, runtime-owned deadlines, and harmless late replies.

type TestProgram = Program<(Effect<Observation>,)>;

use std::{convert::Infallible, time::Duration};

use samara::prelude::*;

/// The provider's vocabulary, written by hand. `Ask` carries the value to
/// echo, how the provider should answer (`None` never replies, `Some(ZERO)`
/// replies inline, otherwise after a delay), and its own reply token.
enum ProbeProtocolMessage {
    Ask {
        value: u64,
        delay: Option<Duration>,
        reply_with: ReplyWith<u64>,
    },
}

/// The request constructor `Command::request_to*` and
/// `PortHandle::request_to*` take.
fn ask(
    value: u64,
    delay: Option<Duration>,
) -> impl FnOnce(ReplyWith<u64>) -> ProbeProtocolMessage + Send + 'static {
    move |reply_with| ProbeProtocolMessage::Ask {
        value,
        delay,
        reply_with,
    }
}

enum ProviderMessage {
    Protocol(ProbeProtocolMessage),
    Reply(ReplyWith<u64>, u64),
}

impl From<ProbeProtocolMessage> for ProviderMessage {
    fn from(message: ProbeProtocolMessage) -> Self {
        Self::Protocol(message)
    }
}

struct Provider;

impl Component for Provider {
    type Requires = ();
    type Config = ();
    type Model = Vec<u64>;
    type Message = ProviderMessage;

    fn init(
        _config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
    ) -> Init<Self::Model, Self::Message> {
        Init::default()
    }

    fn update(
        _config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
        seen: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        match message {
            ProviderMessage::Protocol(ProbeProtocolMessage::Ask {
                value,
                delay,
                reply_with,
            }) => {
                assert_ne!(value, u64::MAX, "provider fault fixture");
                seen.push(value);
                match delay {
                    Some(Duration::ZERO) => Command::reply(reply_with, value),
                    Some(delay) => Command::after(delay, ProviderMessage::Reply(reply_with, value)),
                    None => Command::none(),
                }
            }
            ProviderMessage::Reply(reply_with, value) => Command::reply(reply_with, value),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Observation {
    key: Option<u64>,
    outcome: RequestOutcome<u64>,
}

impl EffectDescriptor for Observation {
    type Output = ();
    type Error = Infallible;
}

enum RequesterMessage {
    Start {
        value: u64,
        delay: Option<Duration>,
        timeout: Option<Duration>,
        key: Option<u64>,
    },
    Outcome(Observation),
}

impl From<RequestOutcome<u64>> for RequesterMessage {
    fn from(outcome: RequestOutcome<u64>) -> Self {
        Self::Outcome(Observation { key: None, outcome })
    }
}

struct Requester;

struct RequesterConfig {
    port: Port<ProbeProtocolMessage>,
    observe: EffectCapability<Observation>,
}

impl Component for Requester {
    type Requires = ();
    type Config = RequesterConfig;
    type Model = Vec<Observation>;
    type Message = RequesterMessage;

    fn init(
        _config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
    ) -> Init<Self::Model, Self::Message> {
        Init::default()
    }

    fn update(
        config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
        model: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        match message {
            RequesterMessage::Start {
                value,
                delay,
                timeout,
                key,
            } => match (timeout, key) {
                (Some(timeout), Some(key)) => Command::request_to_timeout(
                    &config.port,
                    ask(value, delay),
                    timeout,
                    move |outcome| {
                        RequesterMessage::Outcome(Observation {
                            key: Some(key),
                            outcome,
                        })
                    },
                ),
                (Some(timeout), None) => Command::request_to_timeout(
                    &config.port,
                    ask(value, delay),
                    timeout,
                    RequesterMessage::from,
                ),
                (None, _) => {
                    Command::request_to(&config.port, ask(value, delay), RequesterMessage::from)
                }
            },
            RequesterMessage::Outcome(observation) => {
                model.push(observation.clone());
                Command::effect_discarding_outcome(&config.observe, observation)
            }
        }
    }
}

struct Fixture {
    program: TestProgram,
    requester: ComponentRef<Requester>,
    provider: ComponentRef<Provider>,
    port: Port<ProbeProtocolMessage>,
}

fn fixture() -> Fixture {
    let (mut builder, (declared_0,)) = samara::Program::builder::<(samara::Effect<Observation>,)>();
    let port = builder.port::<ProbeProtocolMessage>(PortId::new("probe"));
    let observe = declared_0;
    let provider = builder
        .component::<Provider>(ComponentId::new("provider"), ())
        .finish()
        .expect("component registration");
    builder.connect(&port, &provider.connector());
    let requester = builder
        .component::<Requester>(
            ComponentId::new("requester"),
            RequesterConfig {
                port: port.clone(),
                observe,
            },
        )
        .finish()
        .expect("component registration");
    Fixture {
        program: builder.build().unwrap(),
        requester,
        provider,
        port,
    }
}

fn start(value: u64, delay: Option<Duration>, timeout: Option<Duration>) -> RequesterMessage {
    RequesterMessage::Start {
        value,
        delay,
        timeout,
        key: None,
    }
}

fn settle_observations(runtime: &mut ControlledRuntime) {
    while let Ok(effect) = runtime.next_effect::<Observation>() {
        runtime
            .complete(effect, EffectOutcome::Succeeded(()))
            .unwrap();
    }
    runtime.run_until_idle().unwrap();
}

const SECOND: Duration = Duration::from_secs(1);

#[test]
fn controlled_reply_removes_unused_deadline() {
    let Fixture {
        program, requester, ..
    } = fixture();
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
        .build()
        .unwrap();
    runtime
        .send(&requester, start(7, Some(Duration::ZERO), Some(SECOND)))
        .unwrap();
    runtime.run_until_idle().unwrap();
    settle_observations(&mut runtime);
    assert_eq!(
        runtime.state(&requester).unwrap()[0].outcome,
        RequestOutcome::Replied(7)
    );
    assert_eq!(runtime.pending_work(), PendingWork::default());
    assert!(
        runtime.advance_to_next().is_err(),
        "successful reply removed its deadline"
    );
}

#[test]
fn controlled_timeout_is_one_obligation_with_repeatable_causal_trace() {
    let run = || {
        let Fixture {
            program, requester, ..
        } = fixture();
        let mut runtime = ControlledRuntime::builder(program)
            .control_all()
            .build()
            .unwrap();
        runtime
            .send(&requester, start(7, None, Some(SECOND)))
            .unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(
            runtime.pending_work(),
            PendingWork {
                pending_now: 0,
                pending_later: 1
            }
        );
        runtime.advance(SECOND / 2).unwrap();
        assert!(runtime.state(&requester).unwrap().is_empty());
        runtime.advance_to_next().unwrap();
        settle_observations(&mut runtime);
        assert_eq!(
            runtime.state(&requester).unwrap(),
            &[Observation {
                key: None,
                outcome: RequestOutcome::TimedOut
            }]
        );
        assert_eq!(runtime.pending_work(), PendingWork::default());
        assert!(runtime.advance_to_next().is_err());
        let outcome = runtime
            .trace()
            .iter()
            .find(|record| {
                matches!(
                    record.event,
                    TraceEvent::RequestOutcome {
                        outcome: RequestOutcomeKind::TimedOut,
                        ..
                    }
                )
            })
            .unwrap();
        let parent = &runtime.trace()[outcome.cause.unwrap().get() as usize];
        assert!(matches!(
            parent.event,
            TraceEvent::CommandEmitted {
                kind: TraceCommandKind::Request,
                ..
            }
        ));
        runtime.trace().to_vec()
    };
    assert_eq!(run(), run());
}

#[test]
fn controlled_late_reply_does_not_repeat_outcome_or_cancel_provider_work() {
    let Fixture {
        program,
        requester,
        provider,
        ..
    } = fixture();
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
        .build()
        .unwrap();
    runtime
        .send(&requester, start(7, Some(SECOND * 2), Some(SECOND)))
        .unwrap();
    runtime.advance(SECOND).unwrap();
    settle_observations(&mut runtime);
    assert_eq!(runtime.state(&provider).unwrap(), &[7]);
    assert_eq!(
        runtime.pending_work().pending_later,
        1,
        "provider timer remains owned"
    );
    runtime.advance(SECOND).unwrap();
    assert_eq!(runtime.state(&requester).unwrap().len(), 1);
    assert_eq!(
        runtime.state(&requester).unwrap()[0].outcome,
        RequestOutcome::TimedOut
    );
    assert_eq!(runtime.pending_work(), PendingWork::default());
    assert_eq!(
        runtime
            .trace()
            .iter()
            .filter(|record| matches!(record.event, TraceEvent::LateReplyDropped { .. }))
            .count(),
        1
    );
}

#[test]
fn controlled_zero_and_equal_deadlines_timeout() {
    for duration in [Duration::ZERO, SECOND] {
        let Fixture {
            program,
            requester,
            provider,
            ..
        } = fixture();
        let mut runtime = ControlledRuntime::builder(program)
            .control_all()
            .build()
            .unwrap();
        runtime
            .send(&requester, start(7, Some(duration), Some(duration)))
            .unwrap();
        runtime.advance(duration).unwrap();
        settle_observations(&mut runtime);
        assert_eq!(
            runtime.state(&requester).unwrap()[0].outcome,
            RequestOutcome::TimedOut
        );
        assert_eq!(
            runtime.state(&provider).unwrap(),
            &[7],
            "timeout does not revoke delivery"
        );
        assert_eq!(runtime.pending_work(), PendingWork::default());
    }
}

#[test]
fn controlled_concurrent_requests_preserve_explicit_mapper_context() {
    let Fixture {
        program, requester, ..
    } = fixture();
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
        .build()
        .unwrap();
    for (key, delay) in [(1, None), (2, Some(Duration::ZERO))] {
        runtime
            .send(
                &requester,
                RequesterMessage::Start {
                    value: 7,
                    delay,
                    timeout: Some(SECOND),
                    key: Some(key),
                },
            )
            .unwrap();
    }
    runtime.advance(SECOND).unwrap();
    settle_observations(&mut runtime);
    assert_eq!(
        runtime.state(&requester).unwrap(),
        &[
            Observation {
                key: Some(2),
                outcome: RequestOutcome::Replied(7)
            },
            Observation {
                key: Some(1),
                outcome: RequestOutcome::TimedOut
            },
        ]
    );
}

#[test]
fn controlled_unbounded_requests_and_scope_cancel_keep_existing_semantics() {
    let Fixture {
        program, requester, ..
    } = fixture();
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
        .build()
        .unwrap();
    runtime.send(&requester, start(1, None, None)).unwrap();
    runtime
        .send(&requester, start(2, None, Some(SECOND * 10)))
        .unwrap();
    runtime.advance(SECOND).unwrap();
    assert!(runtime.state(&requester).unwrap().is_empty());
    assert_eq!(runtime.pending_work().pending_later, 2);
    assert!(runtime.cancel().unwrap().is_clean());
}

#[test]
fn controlled_deadline_overflow_faults_explicitly() {
    let Fixture {
        program, requester, ..
    } = fixture();
    let mut runtime = ControlledRuntime::builder(program)
        .control_all()
        .build()
        .unwrap();
    runtime.advance(SECOND).unwrap();
    runtime
        .send(&requester, start(7, None, Some(Duration::MAX)))
        .unwrap();
    let error = runtime.run_until_idle().unwrap_err();
    assert_eq!(error.component(), Some(requester.id()));
    assert!(error.to_string().contains("deadline"));
    assert!(runtime.cancel().unwrap().is_clean());
}

struct Observer(tokio::sync::mpsc::UnboundedSender<Observation>);

impl EffectDriver<Observation> for Observer {
    fn execute(&self, observation: Observation) -> BoxFuture<Result<(), Infallible>> {
        let sender = self.0.clone();
        Box::pin(async move {
            let _ = sender.send(observation);
            Ok(())
        })
    }
}

fn live(program: TestProgram) -> (Runtime, tokio::sync::mpsc::UnboundedReceiver<Observation>) {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    (
        Runtime::builder(program)
            .with_drivers((Observer(sender),))
            .build()
            .unwrap(),
        receiver,
    )
}

#[tokio::test(start_paused = true)]
async fn live_component_timeout_and_late_reply_drain_cleanly() {
    let Fixture {
        program, requester, ..
    } = fixture();
    let (runtime, mut observed) = live(program);
    let handle = runtime.handle(&requester).unwrap();
    let task = runtime.spawn();
    handle
        .send(start(7, Some(SECOND * 2), Some(SECOND)))
        .await
        .unwrap();
    assert_eq!(
        observed.recv().await.unwrap().outcome,
        RequestOutcome::TimedOut
    );
    let report = tokio::time::timeout(SECOND * 10, task.shutdown(Shutdown::Drain))
        .await
        .unwrap()
        .unwrap();
    assert!(report.is_clean());
    assert_eq!(
        observed.recv().await,
        None,
        "late reply creates no second continuation"
    );
}

#[tokio::test(start_paused = true)]
async fn live_reply_cancels_deadline_task_without_delaying_drain() {
    let Fixture {
        program, requester, ..
    } = fixture();
    let (runtime, mut observed) = live(program);
    let handle = runtime.handle(&requester).unwrap();
    let task = runtime.spawn();
    let before = tokio::time::Instant::now();
    handle
        .send(start(7, Some(Duration::ZERO), Some(SECOND * 60)))
        .await
        .unwrap();
    assert_eq!(
        observed.recv().await.unwrap().outcome,
        RequestOutcome::Replied(7)
    );
    assert!(
        tokio::time::timeout(SECOND, task.shutdown(Shutdown::Drain))
            .await
            .unwrap()
            .unwrap()
            .is_clean()
    );
    assert!(before.elapsed() < SECOND);
}

#[tokio::test(start_paused = true)]
async fn host_timeout_includes_time_queued_before_owner_starts() {
    let Fixture { program, port, .. } = fixture();
    let (runtime, _observed) = live(program);
    let port = runtime.port_handle(&port).unwrap();
    let mut request = Box::pin(port.request_to_timeout(ask(7, Some(Duration::ZERO)), SECOND));
    assert!(futures_util::poll!(request.as_mut()).is_pending());
    tokio::time::advance(SECOND * 2).await;
    let task = runtime.spawn();
    assert_eq!(request.await.unwrap(), RequestOutcome::TimedOut);
    assert!(task.shutdown(Shutdown::Drain).await.unwrap().is_clean());
}

#[tokio::test(start_paused = true)]
async fn dropped_host_waiter_keeps_deadline_and_drain_releases_unanswered_request() {
    let Fixture { program, port, .. } = fixture();
    let (runtime, _observed) = live(program);
    let port = runtime.port_handle(&port).unwrap();
    let mut request = Box::pin(port.request_to_timeout(ask(7, None), SECOND));
    assert!(futures_util::poll!(request.as_mut()).is_pending());
    drop(request);
    let task = runtime.spawn();
    let before = tokio::time::Instant::now();
    assert!(
        tokio::time::timeout(SECOND * 10, task.shutdown(Shutdown::Drain))
            .await
            .unwrap()
            .unwrap()
            .is_clean()
    );
    assert!(before.elapsed() >= SECOND);
    assert!(before.elapsed() < SECOND * 10);
}

#[tokio::test(start_paused = true)]
async fn live_zero_and_equal_deadlines_timeout() {
    for duration in [Duration::ZERO, SECOND] {
        let Fixture { program, port, .. } = fixture();
        let (runtime, _observed) = live(program);
        let port = runtime.port_handle(&port).unwrap();
        let task = runtime.spawn();
        assert_eq!(
            port.request_to_timeout(ask(7, Some(duration)), duration)
                .await
                .unwrap(),
            RequestOutcome::TimedOut
        );
        assert!(task.shutdown(Shutdown::Drain).await.unwrap().is_clean());
    }
}

#[tokio::test(start_paused = true)]
async fn live_concurrent_host_timeouts_keep_correlation() {
    let Fixture { program, port, .. } = fixture();
    let (runtime, _observed) = live(program);
    let port = runtime.port_handle(&port).unwrap();
    let task = runtime.spawn();
    let (late, early) = tokio::join!(
        port.request_to_timeout(ask(11, Some(SECOND * 2)), SECOND),
        port.request_to_timeout(ask(22, Some(Duration::ZERO)), SECOND),
    );
    assert_eq!(late.unwrap(), RequestOutcome::TimedOut);
    assert_eq!(early.unwrap(), RequestOutcome::Replied(22));
    assert!(task.shutdown(Shutdown::Drain).await.unwrap().is_clean());
}

#[tokio::test(start_paused = true)]
async fn live_scope_cancel_does_not_manufacture_timeout() {
    let Fixture {
        program,
        requester,
        port,
        ..
    } = fixture();
    let (runtime, mut observed) = live(program);
    let handle = runtime.handle(&requester).unwrap();
    let port = runtime.port_handle(&port).unwrap();
    let task = runtime.spawn();
    handle
        .send(start(7, None, Some(SECOND * 60)))
        .await
        .unwrap();
    let mut request = Box::pin(port.request_to_timeout(ask(8, None), SECOND * 60));
    assert!(futures_util::poll!(request.as_mut()).is_pending());
    tokio::task::yield_now().await;
    assert!(task.shutdown(Shutdown::Cancel).await.unwrap().is_clean());
    assert!(request.await.is_err());
    assert_eq!(observed.recv().await, None);
}

#[tokio::test(start_paused = true)]
async fn live_timeout_preserves_runtime_fault_and_rejects_overflow() {
    let Fixture { program, port, .. } = fixture();
    let (runtime, _observed) = live(program);
    let port = runtime.port_handle(&port).unwrap();
    let task = runtime.spawn();
    let overflow = port
        .request_to_timeout(ask(7, None), Duration::MAX)
        .await
        .unwrap_err();
    assert!(overflow.to_string().contains("deadline"));
    let fault = port
        .request_to_timeout(ask(u64::MAX, None), SECOND)
        .await
        .unwrap_err();
    assert_eq!(fault.component(), Some(&ComponentId::new("provider")));
    assert_eq!(task.shutdown(Shutdown::Cancel).await, Err(fault));
}
