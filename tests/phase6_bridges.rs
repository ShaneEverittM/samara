use std::{convert::Infallible, net::SocketAddr};

use bytes::Bytes;
use samara::prelude::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone, Debug, PartialEq, Eq)]
enum StreamObservation {
    Item(u64),
    Ended,
}

#[derive(Clone, Debug)]
struct ObserveStream(StreamObservation);

impl EffectDescriptor for ObserveStream {
    type Output = ();
    type Error = Infallible;
}

struct StreamObserver {
    observations: tokio::sync::mpsc::UnboundedSender<StreamObservation>,
}

impl EffectDriver<ObserveStream> for StreamObserver {
    fn execute(&self, descriptor: ObserveStream) -> BoxFuture<Result<(), Infallible>> {
        let observations = self.observations.clone();
        Box::pin(async move {
            let _ = observations.send(descriptor.0);
            Ok(())
        })
    }
}

enum StreamMessage {
    Event(SourceEvent<u64, Infallible>),
    Reopen,
    Observed,
}

struct StreamProbe {
    stream: StreamDescriptor<u64>,
}

struct StreamModel {
    desired: bool,
}

impl Component for StreamProbe {
    type Model = StreamModel;
    type Message = StreamMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(StreamModel { desired: true })
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            StreamMessage::Event(SourceEvent::Item(value)) => {
                Command::effect_with(ObserveStream(StreamObservation::Item(value)), |_| {
                    StreamMessage::Observed
                })
            }
            StreamMessage::Event(SourceEvent::Ended) => {
                model.desired = false;
                Command::effect_with(ObserveStream(StreamObservation::Ended), |_| {
                    StreamMessage::Observed
                })
            }
            StreamMessage::Event(SourceEvent::Failed(never)) => match never {},
            StreamMessage::Reopen => {
                model.desired = true;
                Command::none()
            }
            StreamMessage::Observed => Command::none(),
        }
    }

    fn subscriptions(&self, model: &Self::Model) -> Subscriptions<Self::Message> {
        if model.desired {
            Subscriptions::one(Subscription::source_with(
                SubscriptionId::new("stream"),
                self.stream.clone(),
                StreamMessage::Event,
            ))
        } else {
            Subscriptions::none()
        }
    }
}

fn stream_program(
    descriptor: StreamDescriptor<u64>,
    count: usize,
) -> (Program, Vec<ComponentRef<StreamProbe>>) {
    let mut program = Program::builder();
    let components = (0..count)
        .map(|index| {
            program.component(
                ComponentId::new(format!("stream-{index}")),
                StreamProbe {
                    stream: descriptor.clone(),
                },
            )
        })
        .collect();
    (program.build().expect("valid stream program"), components)
}

#[tokio::test]
async fn phase6_mpsc_closure_ends_once() {
    let descriptor = StreamDescriptor::named("bridge/stream");
    let (sender, receiver) = tokio::sync::mpsc::channel(4);
    sender.send(1).await.expect("receiver retained");
    sender.send(2).await.expect("receiver retained");
    drop(sender);
    let (observed, mut observations) = tokio::sync::mpsc::unbounded_channel();
    let (program, _) = stream_program(descriptor.clone(), 1);
    let runtime = LiveRuntime::builder(program)
        .bind_mpsc(descriptor, receiver)
        .bind_effect::<ObserveStream, _>(StreamObserver {
            observations: observed,
        })
        .build()
        .expect("valid live bridge");
    let task = runtime.spawn();

    assert_eq!(observations.recv().await, Some(StreamObservation::Item(1)));
    assert_eq!(observations.recv().await, Some(StreamObservation::Item(2)));
    assert_eq!(observations.recv().await, Some(StreamObservation::Ended));
    let report = task.shutdown(Shutdown::Drain).await.expect("clean drain");
    assert!(report.is_clean());
    assert!(observations.try_recv().is_err());
}

#[tokio::test]
async fn phase6_mpsc_duplicate_or_reactivation_faults() {
    let descriptor = StreamDescriptor::named("bridge/duplicate");
    let (_sender, receiver) = tokio::sync::mpsc::channel::<u64>(1);
    let (observed, _observations) = tokio::sync::mpsc::unbounded_channel();
    let (program, components) = stream_program(descriptor.clone(), 2);
    let runtime = LiveRuntime::builder(program)
        .bind_mpsc(descriptor, receiver)
        .bind_effect::<ObserveStream, _>(StreamObserver {
            observations: observed,
        })
        .build()
        .expect("one exact binding is valid assembly");
    let handle = runtime
        .handle(&components[0])
        .expect("registered stream Component");
    let task = runtime.spawn();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if handle.send(StreamMessage::Reopen).await.is_err() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("duplicate claimant faults promptly");
    let error = task
        .shutdown(Shutdown::Drain)
        .await
        .expect_err("a second active claimant exhausts the one-shot receiver");
    assert!(error.descriptor_type().is_some());

    let descriptor = StreamDescriptor::named("bridge/reactivate");
    let (sender, receiver) = tokio::sync::mpsc::channel::<u64>(1);
    drop(sender);
    let (observed, mut observations) = tokio::sync::mpsc::unbounded_channel();
    let (program, components) = stream_program(descriptor.clone(), 1);
    let runtime = LiveRuntime::builder(program)
        .bind_mpsc(descriptor, receiver)
        .bind_effect::<ObserveStream, _>(StreamObserver {
            observations: observed,
        })
        .build()
        .expect("valid one-shot binding");
    let handle = runtime
        .handle(&components[0])
        .expect("registered stream Component");
    let task = runtime.spawn();
    assert_eq!(observations.recv().await, Some(StreamObservation::Ended));
    handle
        .send(StreamMessage::Reopen)
        .await
        .expect("reactivation Message accepted");
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if handle.send(StreamMessage::Reopen).await.is_err() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("reactivation faults promptly");
    let error = task
        .shutdown(Shutdown::Drain)
        .await
        .expect_err("reactivating a consumed receiver faults explicitly");
    assert!(error.descriptor_type().is_some());
    assert!(observations.try_recv().is_err());
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TcpObservation {
    Bytes(Bytes),
    Failed(TcpErrorKind),
    Ended,
}

#[derive(Clone, Debug)]
struct ObserveTcp(TcpObservation);

impl EffectDescriptor for ObserveTcp {
    type Output = ();
    type Error = Infallible;
}

struct TcpObserver {
    observations: tokio::sync::mpsc::UnboundedSender<TcpObservation>,
}

impl EffectDriver<ObserveTcp> for TcpObserver {
    fn execute(&self, descriptor: ObserveTcp) -> BoxFuture<Result<(), Infallible>> {
        let observations = self.observations.clone();
        Box::pin(async move {
            let _ = observations.send(descriptor.0);
            Ok(())
        })
    }
}

enum TcpMessage {
    Event(SourceEvent<Bytes, TcpError>),
    Observed,
}

struct TcpProbe {
    tcp: TcpBytes,
}

impl Component for TcpProbe {
    type Model = bool;
    type Message = TcpMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(true)
    }

    fn update(&self, active: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        let observation = match message {
            TcpMessage::Event(SourceEvent::Item(bytes)) => TcpObservation::Bytes(bytes),
            TcpMessage::Event(SourceEvent::Failed(error)) => {
                *active = false;
                TcpObservation::Failed(error.kind())
            }
            TcpMessage::Event(SourceEvent::Ended) => {
                *active = false;
                TcpObservation::Ended
            }
            TcpMessage::Observed => return Command::none(),
        };
        Command::effect_with(ObserveTcp(observation), |_| TcpMessage::Observed)
    }

    fn subscriptions(&self, active: &Self::Model) -> Subscriptions<Self::Message> {
        if *active {
            Subscriptions::one(Subscription::source_with(
                SubscriptionId::new("tcp"),
                self.tcp.clone(),
                TcpMessage::Event,
            ))
        } else {
            Subscriptions::none()
        }
    }
}

fn tcp_program(endpoint: SocketAddr) -> (Program, ComponentRef<TcpProbe>) {
    let mut program = Program::builder();
    let probe = program.component(
        ComponentId::new("tcp-probe"),
        TcpProbe {
            tcp: TcpBytes::connect(endpoint),
        },
    );
    (program.build().expect("valid TCP program"), probe)
}

fn tcp_runtime(
    endpoint: SocketAddr,
) -> (
    LiveRuntime,
    tokio::sync::mpsc::UnboundedReceiver<TcpObservation>,
) {
    let (program, _probe) = tcp_program(endpoint);
    let (observed, observations) = tokio::sync::mpsc::unbounded_channel();
    let runtime = LiveRuntime::builder(program)
        .bind_tcp()
        .bind_effect::<ObserveTcp, _>(TcpObserver {
            observations: observed,
        })
        .build()
        .expect("valid TCP bindings");
    (runtime, observations)
}

#[test]
fn phase6_tcp_failure_is_constructible_and_scriptable_in_controlled_execution() {
    let endpoint = SocketAddr::from(([127, 0, 0, 1], 9));
    let (program, probe) = tcp_program(endpoint);
    let mut runtime = ControlledRuntime::builder(program)
        .control_source::<TcpBytes>()
        .control_effect::<ObserveTcp>()
        .build()
        .expect("valid controlled TCP boundary");
    let failure = TcpError::new(TcpErrorKind::Read, "controlled reset");
    assert_eq!(failure.message(), "controlled reset");

    runtime
        .emit_source::<TcpProbe, TcpBytes>(
            &probe,
            &SubscriptionId::new("tcp"),
            SourceEvent::Failed(failure),
        )
        .expect("typed TCP failure is injectable");
    runtime
        .run_until_idle()
        .expect("failure reaches the Component");
    let pending = runtime
        .next_effect::<ObserveTcp>()
        .expect("failure observation emitted");
    assert_eq!(pending.intent.0, TcpObservation::Failed(TcpErrorKind::Read));
    runtime
        .complete(pending, EffectOutcome::Succeeded(()))
        .expect("observation completion accepted");
    runtime.run_until_idle().expect("completion mapped");
    assert!(!runtime.state(&probe).expect("TCP model"));
    assert!(runtime.cancel().expect("clean controlled close").is_clean());
}

#[tokio::test]
async fn phase6_tcp_emits_bytes_and_maps_connect_read_and_peer_eof() -> Result<(), RuntimeError> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback listener");
    let endpoint = listener.local_addr().expect("numeric endpoint");
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("one connection");
        stream.write_all(b"abc").await.expect("write bytes");
        stream.shutdown().await.expect("peer EOF");
    });
    let (runtime, mut observations) = tcp_runtime(endpoint);
    let task = runtime.spawn();

    let mut received = Vec::new();
    loop {
        match observations.recv().await.expect("TCP observation") {
            TcpObservation::Bytes(bytes) => received.extend_from_slice(&bytes),
            TcpObservation::Ended => break,
            TcpObservation::Failed(kind) => panic!("unexpected TCP failure: {kind:?}"),
        }
    }
    assert_eq!(received, b"abc");
    server.await.expect("server task");
    assert!(task.shutdown(Shutdown::Drain).await?.is_clean());

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve endpoint");
    let endpoint = listener.local_addr().expect("numeric endpoint");
    drop(listener);
    let (runtime, mut observations) = tcp_runtime(endpoint);
    let task = runtime.spawn();
    assert_eq!(
        observations.recv().await,
        Some(TcpObservation::Failed(TcpErrorKind::Connect))
    );
    assert!(task.shutdown(Shutdown::Drain).await?.is_clean());

    Ok(())
}

#[tokio::test]
async fn phase6_tcp_cancellation_closes_connection() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback listener");
    let endpoint = listener.local_addr().expect("numeric endpoint");
    let (runtime, _observations) = tcp_runtime(endpoint);
    let task = runtime.spawn();
    let (mut server, _) = listener.accept().await.expect("one connection");

    let report = task.shutdown(Shutdown::Cancel).await.expect("clean cancel");
    assert!(report.is_clean());
    let mut byte = [0_u8; 1];
    assert_eq!(server.read(&mut byte).await.expect("socket close"), 0);
}

#[tokio::test]
async fn phase6_tcp_has_no_hidden_retry_or_framing() -> Result<(), RuntimeError> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve endpoint");
    let endpoint = listener.local_addr().expect("numeric endpoint");
    drop(listener);
    let (runtime, mut observations) = tcp_runtime(endpoint);
    let task = runtime.spawn();
    assert_eq!(
        observations.recv().await,
        Some(TcpObservation::Failed(TcpErrorKind::Connect))
    );

    let listener = tokio::net::TcpListener::bind(endpoint)
        .await
        .expect("endpoint becomes available after the one failed attempt");
    assert!(task.shutdown(Shutdown::Drain).await?.is_clean());
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), listener.accept())
            .await
            .is_err(),
        "the terminal Driver must not retry after failure"
    );
    Ok(())
}
