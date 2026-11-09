use std::{
    convert::Infallible,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use samara::prelude::*;

/// The host's vocabulary, written by hand. A request variant carries its own
/// reply token, which is what types the reply.
#[derive(Debug)]
enum HostMessage {
    Set(u64),
    RecordIngress(u64),
    Read(ReplyWith<u64>),
    Delayed(u64, ReplyWith<u64>),
    Held(u64, ReplyWith<u64>),
    Never(ReplyWith<()>),
    Crash(ReplyWith<()>),
}

/// The vocabulary of a Port whose binding conversion panics for every message.
enum BindingMessage {
    Signal,
    Ask(ReplyWith<()>),
}

struct PanickingBindingProviderMessage;

struct PanickingBindingProvider;

impl Component for PanickingBindingProvider {
    type Config = ();
    type Model = ();
    type Message = PanickingBindingProviderMessage;

    fn init(_config: &Self::Config) -> Init<Self::Model, Self::Message> {
        Init::new(())
    }

    fn update(
        _config: &Self::Config,
        _model: &mut Self::Model,
        _message: Self::Message,
    ) -> Command<Self::Message> {
        Command::none()
    }
}

enum ProviderMessage {
    Host(HostMessage),
    RecordIngressDirect(u64),
    CompleteDelayed {
        reply_with: ReplyWith<u64>,
        value: u64,
    },
    Signalled,
}

struct Provider;

struct ProviderConfig {
    ingress_observed: EffectCapability<IngressObserved>,
    request_observed: EffectCapability<RequestObserved>,
    await_release: EffectCapability<AwaitRelease>,
}

impl Component for Provider {
    type Config = ProviderConfig;
    type Model = u64;
    type Message = ProviderMessage;

    fn init(_config: &Self::Config) -> Init<Self::Model, Self::Message> {
        Init::new(0)
    }

    fn update(
        config: &Self::Config,
        model: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        match message {
            ProviderMessage::Host(HostMessage::Set(value)) => {
                *model = value;
                Command::none()
            }
            ProviderMessage::Host(HostMessage::RecordIngress(value))
            | ProviderMessage::RecordIngressDirect(value) => {
                Command::effect_discarding_outcome(&config.ingress_observed, IngressObserved(value))
            }
            ProviderMessage::Host(HostMessage::Read(reply_with)) => {
                Command::reply(reply_with, *model)
            }
            ProviderMessage::Host(HostMessage::Delayed(value, reply_with)) => {
                let delay = if value == 11 {
                    Duration::from_millis(50)
                } else {
                    Duration::from_millis(10)
                };
                Command::batch([
                    Command::effect_with(&config.request_observed, RequestObserved, |_| {
                        ProviderMessage::Signalled
                    }),
                    Command::after(
                        delay,
                        ProviderMessage::CompleteDelayed { reply_with, value },
                    ),
                ])
            }
            ProviderMessage::Host(HostMessage::Held(value, reply_with)) => {
                Command::effect_with(&config.await_release, AwaitRelease(value), move |outcome| {
                    match outcome {
                        EffectOutcome::Succeeded(value) => {
                            ProviderMessage::CompleteDelayed { reply_with, value }
                        }
                        EffectOutcome::Failed(never) => match never {},
                        EffectOutcome::Cancelled(_) => ProviderMessage::Signalled,
                    }
                })
            }
            ProviderMessage::Host(HostMessage::Never(_reply_with)) => {
                Command::effect_with(&config.request_observed, RequestObserved, |_| {
                    ProviderMessage::Signalled
                })
            }
            ProviderMessage::Host(HostMessage::Crash(_reply_with)) => {
                panic!("provider crashed while handling an external request")
            }
            ProviderMessage::CompleteDelayed { reply_with, value } => {
                Command::reply(reply_with, value)
            }
            ProviderMessage::Signalled => Command::none(),
        }
    }
}

#[derive(Debug)]
struct RequestObserved;

impl EffectDescriptor for RequestObserved {
    type Output = ();
    type Error = Infallible;
}

struct ObservationDriver {
    observed: tokio::sync::mpsc::UnboundedSender<()>,
}

#[derive(Debug)]
struct AwaitRelease(u64);

impl EffectDescriptor for AwaitRelease {
    type Output = u64;
    type Error = Infallible;
}

struct ReleaseDriver {
    started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

impl EffectDriver<AwaitRelease> for ReleaseDriver {
    fn execute(&self, descriptor: AwaitRelease) -> BoxFuture<Result<u64, Infallible>> {
        let started = self.started.lock().expect("started lock").take();
        let release = self.release.lock().expect("release lock").take();
        Box::pin(async move {
            if let Some(started) = started {
                let _ = started.send(());
            }
            if let Some(release) = release {
                let _ = release.await;
            }
            Ok(descriptor.0)
        })
    }
}

struct ImmediateReleaseDriver;

impl EffectDriver<AwaitRelease> for ImmediateReleaseDriver {
    fn execute(&self, descriptor: AwaitRelease) -> BoxFuture<Result<u64, Infallible>> {
        Box::pin(async move { Ok(descriptor.0) })
    }
}

#[derive(Debug)]
struct IngressObserved(u64);

impl EffectDescriptor for IngressObserved {
    type Output = ();
    type Error = Infallible;
}

struct IngressObserver {
    observed: tokio::sync::mpsc::UnboundedSender<u64>,
}

impl EffectDriver<IngressObserved> for IngressObserver {
    fn execute(&self, descriptor: IngressObserved) -> BoxFuture<Result<(), Infallible>> {
        let observed = self.observed.clone();
        Box::pin(async move {
            let _ = observed.send(descriptor.0);
            Ok(())
        })
    }
}

impl EffectDriver<RequestObserved> for ObservationDriver {
    fn execute(&self, _descriptor: RequestObserved) -> BoxFuture<Result<(), Infallible>> {
        let observed = self.observed.clone();
        Box::pin(async move {
            let _ = observed.send(());
            Ok(())
        })
    }
}

fn program() -> (Program, Port<HostMessage>) {
    let (program, port, _conversions) = counted_binding_program();
    (program, port)
}

/// A Program whose Port binding counts every conversion it performs, so a
/// test can tell whether a rejected ingress reached the binding at all.
fn counted_binding_program() -> (Program, Port<HostMessage>, Arc<AtomicUsize>) {
    let mut builder = Program::builder();
    let port = builder.port::<HostMessage>(PortId::new("host"));
    let ingress_observed = builder.effect::<IngressObserved>();
    let request_observed = builder.effect::<RequestObserved>();
    let await_release = builder.effect::<AwaitRelease>();
    let provider = builder.component::<Provider>(
        ComponentId::new("provider"),
        ProviderConfig {
            ingress_observed,
            request_observed,
            await_release,
        },
    );
    let conversions = Arc::new(AtomicUsize::new(0));
    let counter = conversions.clone();
    builder.connect(
        &port,
        &provider.accepting(move |message| {
            counter.fetch_add(1, Ordering::SeqCst);
            ProviderMessage::Host(message)
        }),
    );
    (builder.build().expect("valid Program"), port, conversions)
}

/// A Program whose Port binding panics only when converting `Set(0)`, so
/// earlier sends land before the poisoned one faults the runtime.
fn selectively_panicking_binding_program() -> (Program, Port<HostMessage>) {
    let mut builder = Program::builder();
    let port = builder.port::<HostMessage>(PortId::new("host"));
    let ingress_observed = builder.effect::<IngressObserved>();
    let request_observed = builder.effect::<RequestObserved>();
    let await_release = builder.effect::<AwaitRelease>();
    let provider = builder.component::<Provider>(
        ComponentId::new("provider"),
        ProviderConfig {
            ingress_observed,
            request_observed,
            await_release,
        },
    );
    builder.connect(
        &port,
        &provider.accepting(|message| match message {
            HostMessage::Set(0) => panic!("send binding conversion panic"),
            other => ProviderMessage::Host(other),
        }),
    );
    (builder.build().expect("valid Program"), port)
}

fn panicking_binding_program() -> (Program, Port<BindingMessage>) {
    let mut builder = Program::builder();
    let port = builder.port::<BindingMessage>(PortId::new("panicking-binding"));
    let provider = builder
        .component::<PanickingBindingProvider>(ComponentId::new("panicking-binding-provider"), ());
    builder.connect(
        &port,
        &provider.accepting(|message| {
            if let BindingMessage::Ask(reply_with) = message {
                let _ = reply_with;
            }
            panic!("provider binding conversion panic")
        }),
    );
    (builder.build().expect("valid binding Program"), port)
}

fn runtime_with_observer(program: Program) -> (Runtime, tokio::sync::mpsc::UnboundedReceiver<()>) {
    let (observed, observations) = tokio::sync::mpsc::unbounded_channel();
    let (ingress_observed, _ingress_observations) = tokio::sync::mpsc::unbounded_channel();
    let runtime = Runtime::builder(program)
        .bind_effect::<RequestObserved, _>(ObservationDriver { observed })
        .bind_effect::<AwaitRelease, _>(ImmediateReleaseDriver)
        .bind_effect::<IngressObserved, _>(IngressObserver {
            observed: ingress_observed,
        })
        .build()
        .expect("valid live bindings");
    (runtime, observations)
}

fn runtime_with_default_effects(program: Program) -> Runtime {
    let (request_observed, _request_observations) = tokio::sync::mpsc::unbounded_channel();
    let (ingress_observed, _ingress_observations) = tokio::sync::mpsc::unbounded_channel();
    Runtime::builder(program)
        .bind_effect::<RequestObserved, _>(ObservationDriver {
            observed: request_observed,
        })
        .bind_effect::<AwaitRelease, _>(ImmediateReleaseDriver)
        .bind_effect::<IngressObserved, _>(IngressObserver {
            observed: ingress_observed,
        })
        .build()
        .expect("valid default live bindings")
}

#[test]
fn live_port_handle_rejects_a_port_from_another_program() {
    let (program, _) = program();
    let runtime = runtime_with_default_effects(program);

    let mut foreign_builder = Program::builder();
    let foreign_port = foreign_builder.port::<HostMessage>(PortId::new("host"));

    let error = runtime
        .port_handle(&foreign_port)
        .expect_err("a foreign Port cannot become a live capability");
    assert!(error.to_string().contains("not part of this Program"));
}

#[tokio::test]
async fn live_port_handle_sends_and_returns_typed_request_replies() -> Result<(), RuntimeError> {
    let (program, port, conversions) = counted_binding_program();
    let runtime = runtime_with_default_effects(program);
    let host = runtime.port_handle(&port)?;
    let task = runtime.spawn();

    let sends = host.clone();
    sends.send(HostMessage::Set(41)).await?;
    assert_eq!(host.request_to(HostMessage::Read).await?, 41);
    assert_eq!(
        conversions.load(Ordering::SeqCst),
        2,
        "the send and the request each crossed the binding once"
    );

    assert!(task.shutdown(Shutdown::Drain).await?.is_clean());
    assert!(host.send(HostMessage::Set(42)).await.is_err());
    assert!(host.request_to(HostMessage::Read).await.is_err());
    assert_eq!(
        conversions.load(Ordering::SeqCst),
        2,
        "ingress rejected after shutdown never reaches the binding conversion"
    );
    Ok(())
}

#[tokio::test]
async fn cloned_port_handles_deliver_each_send_exactly_once() -> Result<(), RuntimeError> {
    let mut builder = Program::builder();
    let port = builder.port::<HostMessage>(PortId::new("cloned-host"));
    let ingress_observed = builder.effect::<IngressObserved>();
    let request_observed = builder.effect::<RequestObserved>();
    let await_release = builder.effect::<AwaitRelease>();
    let provider = builder.component::<Provider>(
        ComponentId::new("cloned-provider"),
        ProviderConfig {
            ingress_observed,
            request_observed,
            await_release,
        },
    );
    builder.connect(&port, &provider.accepting(ProviderMessage::Host));
    let (observed, mut observations) = tokio::sync::mpsc::unbounded_channel();
    let (request_observed, _request_observations) = tokio::sync::mpsc::unbounded_channel();
    let runtime = Runtime::builder(builder.build().expect("valid cloned-handle Program"))
        .bind_effect::<IngressObserved, _>(IngressObserver { observed })
        .bind_effect::<RequestObserved, _>(ObservationDriver {
            observed: request_observed,
        })
        .bind_effect::<AwaitRelease, _>(ImmediateReleaseDriver)
        .build()?;
    let host = runtime.port_handle(&port)?;
    let task = runtime.spawn();
    let count = 32_u64;
    let mut sends = tokio::task::JoinSet::new();

    for value in 0..count {
        let host = host.clone();
        sends.spawn(async move { host.send(HostMessage::RecordIngress(value)).await });
    }
    while let Some(result) = sends.join_next().await {
        result.expect("send task did not panic")?;
    }
    assert!(task.shutdown(Shutdown::Drain).await?.is_clean());

    let mut actual = Vec::new();
    while let Ok(value) = observations.try_recv() {
        actual.push(value);
    }
    actual.sort_unstable();
    assert_eq!(actual, (0..count).collect::<Vec<_>>());
    Ok(())
}

#[tokio::test]
async fn concurrent_external_requests_keep_their_own_typed_reply() -> Result<(), RuntimeError> {
    let (program, port) = program();
    let (runtime, _observations) = runtime_with_observer(program);
    let host = runtime.port_handle(&port)?;
    let task = runtime.spawn();

    // The second request replies first; a FIFO-only implementation would swap
    // these values instead of following each runtime-owned correlation.
    let (left, right) = tokio::join!(
        host.request_to(|reply_with| HostMessage::Delayed(11, reply_with)),
        host.request_to(|reply_with| HostMessage::Delayed(29, reply_with))
    );
    assert_eq!(left?, 11);
    assert_eq!(right?, 29);

    assert!(task.shutdown(Shutdown::Drain).await?.is_clean());
    Ok(())
}

#[tokio::test]
async fn drain_preserves_an_admitted_external_request() -> Result<(), RuntimeError> {
    let (program, port) = program();
    let (runtime, mut observations) = runtime_with_observer(program);
    let host = runtime.port_handle(&port)?;
    let task = runtime.spawn();

    let request = tokio::spawn(async move {
        host.request_to(|reply_with| HostMessage::Delayed(73, reply_with))
            .await
    });
    observations
        .recv()
        .await
        .expect("the provider accepted the request before Drain");

    let report = task.shutdown(Shutdown::Drain).await?;
    assert_eq!(request.await.expect("request task did not panic")?, 73);
    assert!(report.is_clean());
    Ok(())
}

#[tokio::test]
async fn cancel_wakes_an_external_request_waiter_without_inventing_a_reply()
-> Result<(), RuntimeError> {
    let (program, port) = program();
    let (runtime, mut observations) = runtime_with_observer(program);
    let host = runtime.port_handle(&port)?;
    let task = runtime.spawn();

    let request = tokio::spawn(async move { host.request_to(HostMessage::Never).await });
    observations
        .recv()
        .await
        .expect("the provider accepted the request before Cancel");

    assert!(task.shutdown(Shutdown::Cancel).await?.is_clean());
    let error = request
        .await
        .expect("request task did not panic")
        .expect_err("Cancel cannot manufacture a reply");
    assert!(error.to_string().contains("ended before a reply"));
    Ok(())
}

#[tokio::test]
async fn dropping_an_unpolled_request_admits_no_runtime_work() -> Result<(), RuntimeError> {
    let (program, port) = program();
    let runtime = runtime_with_default_effects(program);
    let host = runtime.port_handle(&port)?;
    let task = runtime.spawn();

    let request = host.request_to(HostMessage::Never);
    drop(request);

    let report = tokio::time::timeout(Duration::from_secs(1), task.shutdown(Shutdown::Drain))
        .await
        .expect("an unpolled future cannot keep Drain open")?;
    assert!(report.is_clean());
    Ok(())
}

#[tokio::test]
async fn dropping_the_waiter_does_not_cancel_runtime_owned_request_work() -> Result<(), RuntimeError>
{
    let (program, port) = program();
    let (started, request_started) = tokio::sync::oneshot::channel();
    let (release, request_release) = tokio::sync::oneshot::channel();
    let (request_observed, _request_observations) = tokio::sync::mpsc::unbounded_channel();
    let (ingress_observed, _ingress_observations) = tokio::sync::mpsc::unbounded_channel();
    let runtime = Runtime::builder(program)
        .bind_effect::<AwaitRelease, _>(ReleaseDriver {
            started: Mutex::new(Some(started)),
            release: Mutex::new(Some(request_release)),
        })
        .bind_effect::<RequestObserved, _>(ObservationDriver {
            observed: request_observed,
        })
        .bind_effect::<IngressObserved, _>(IngressObserver {
            observed: ingress_observed,
        })
        .build()?;
    let host = runtime.port_handle(&port)?;
    let task = runtime.spawn();

    let request = tokio::spawn(async move {
        host.request_to(|reply_with| HostMessage::Held(5, reply_with))
            .await
    });
    request_started
        .await
        .expect("the provider issued held request work");
    request.abort();

    let mut shutdown = tokio::spawn(task.shutdown(Shutdown::Drain));
    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut shutdown)
            .await
            .is_err(),
        "dropping the waiter must not let Drain forget the held request"
    );
    release.send(()).expect("held Driver is still owned");
    let report = tokio::time::timeout(Duration::from_secs(1), shutdown)
        .await
        .expect("Drain completes after the held request replies")
        .expect("shutdown task did not panic")?;
    assert!(report.is_clean());
    Ok(())
}

#[tokio::test]
async fn dropping_the_runtime_owner_wakes_an_external_request_waiter() -> Result<(), RuntimeError> {
    let (program, port) = program();
    let (runtime, mut observations) = runtime_with_observer(program);
    let host = runtime.port_handle(&port)?;
    let task = runtime.spawn();

    let request = tokio::spawn(async move { host.request_to(HostMessage::Never).await });
    observations
        .recv()
        .await
        .expect("the provider accepted the request before owner closure");
    drop(task);

    let error = tokio::time::timeout(Duration::from_secs(1), request)
        .await
        .expect("owner closure wakes the request waiter")
        .expect("request task did not panic")
        .expect_err("owner closure cannot manufacture a reply");
    assert!(error.to_string().contains("ended before a reply"));
    Ok(())
}

#[tokio::test]
async fn component_and_port_ingress_share_one_shutdown_cutoff() -> Result<(), RuntimeError> {
    let mut builder = Program::builder();
    let port = builder.port::<HostMessage>(PortId::new("race-host"));
    let ingress_observed = builder.effect::<IngressObserved>();
    let request_observed = builder.effect::<RequestObserved>();
    let await_release = builder.effect::<AwaitRelease>();
    let provider = builder.component::<Provider>(
        ComponentId::new("race-provider"),
        ProviderConfig {
            ingress_observed,
            request_observed,
            await_release,
        },
    );
    builder.connect(&port, &provider.accepting(ProviderMessage::Host));
    let (observed, mut observations) = tokio::sync::mpsc::unbounded_channel();
    let (request_observed, _request_observations) = tokio::sync::mpsc::unbounded_channel();
    let runtime = Runtime::builder(builder.build().expect("valid race Program"))
        .bind_effect::<IngressObserved, _>(IngressObserver { observed })
        .bind_effect::<RequestObserved, _>(ObservationDriver {
            observed: request_observed,
        })
        .bind_effect::<AwaitRelease, _>(ImmediateReleaseDriver)
        .build()?;
    let component = runtime.handle(&provider)?;
    let protocol = runtime.port_handle(&port)?;
    let task = runtime.spawn();
    let barrier = Arc::new(tokio::sync::Barrier::new(3));

    let component_ingress = {
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            component
                .send(ProviderMessage::RecordIngressDirect(1))
                .await
        })
    };
    let port_ingress = {
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            protocol.send(HostMessage::RecordIngress(2)).await
        })
    };
    let shutdown = tokio::spawn(async move {
        barrier.wait().await;
        task.shutdown(Shutdown::Drain).await
    });

    let component_result = component_ingress.await.expect("Component ingress task");
    let port_result = port_ingress.await.expect("Port ingress task");
    assert!(
        shutdown.await.expect("shutdown task")?.is_clean(),
        "the forced race must still close cleanly"
    );

    let mut expected = Vec::new();
    if component_result.is_ok() {
        expected.push(1);
    }
    if port_result.is_ok() {
        expected.push(2);
    }
    let mut actual = Vec::new();
    while let Ok(value) = observations.try_recv() {
        actual.push(value);
    }
    expected.sort_unstable();
    actual.sort_unstable();
    assert_eq!(actual, expected, "every admitted ingress is drained once");
    Ok(())
}

#[tokio::test]
async fn a_runtime_fault_is_returned_to_the_external_request_waiter() -> Result<(), RuntimeError> {
    let (program, port) = program();
    let runtime = runtime_with_default_effects(program);
    let host = runtime.port_handle(&port)?;
    let task = runtime.spawn();

    let request_error = host
        .request_to(HostMessage::Crash)
        .await
        .expect_err("the provider transition faults the runtime");
    assert!(request_error.to_string().contains("transition panicked"));
    let later_error = host
        .send(HostMessage::Set(1))
        .await
        .expect_err("later Port ingress observes the stored fault");
    assert_eq!(request_error.to_string(), later_error.to_string());

    let shutdown_error = task
        .shutdown(Shutdown::Cancel)
        .await
        .expect_err("the owner observes the same runtime fault");
    assert_eq!(request_error.to_string(), shutdown_error.to_string());
    Ok(())
}

#[tokio::test]
async fn request_conversion_panics_are_contained_as_runtime_faults() -> Result<(), RuntimeError> {
    let (program, port) = program();
    let runtime = runtime_with_default_effects(program);
    let host = runtime.port_handle(&port)?;
    let task = runtime.spawn();

    // The constructor runs inside the runtime, when the request is processed,
    // so its panic is contained as a runtime fault rather than unwinding the
    // host task.
    let request_error = host
        .request_to(|_reply_with: ReplyWith<()>| -> HostMessage {
            panic!("request conversion panic")
        })
        .await
        .expect_err("conversion panic faults the runtime");
    assert!(
        request_error
            .to_string()
            .contains("protocol conversion panicked"),
        "{request_error}"
    );

    let shutdown_error = task
        .shutdown(Shutdown::Cancel)
        .await
        .expect_err("the owner observes the conversion fault");
    assert_eq!(request_error.to_string(), shutdown_error.to_string());
    Ok(())
}

#[tokio::test]
async fn send_binding_conversion_panics_are_contained_after_earlier_sends_land()
-> Result<(), RuntimeError> {
    let (program, port) = selectively_panicking_binding_program();
    let runtime = runtime_with_default_effects(program);
    let host = runtime.port_handle(&port)?;
    let task = runtime.spawn();

    // A send carries a finished value, so the only conversion that can panic
    // for it is the Port's binding. Earlier sends through the same binding
    // land; the poisoned one faults the runtime when it is processed.
    host.send(HostMessage::Set(41)).await?;
    assert_eq!(host.request_to(HostMessage::Read).await?, 41);
    host.send(HostMessage::Set(0)).await?;
    let error = task
        .shutdown(Shutdown::Drain)
        .await
        .expect_err("send binding conversion panic faults the runtime");
    assert!(
        error.to_string().contains("binding conversion panicked"),
        "{error}"
    );
    Ok(())
}

#[tokio::test]
async fn send_binding_panics_are_contained_as_runtime_faults() -> Result<(), RuntimeError> {
    let (program, port) = panicking_binding_program();
    let runtime = Runtime::builder(program).build()?;
    let host = runtime.port_handle(&port)?;
    let task = runtime.spawn();

    host.send(BindingMessage::Signal).await?;
    let error = task
        .shutdown(Shutdown::Drain)
        .await
        .expect_err("send binding panic faults the runtime");
    assert!(
        error.to_string().contains("binding conversion panicked"),
        "{error}"
    );
    Ok(())
}

#[tokio::test]
async fn request_binding_panics_are_contained_as_runtime_faults() -> Result<(), RuntimeError> {
    let (program, port) = panicking_binding_program();
    let runtime = Runtime::builder(program).build()?;
    let host = runtime.port_handle(&port)?;
    let task = runtime.spawn();

    let request_error = host
        .request_to(BindingMessage::Ask)
        .await
        .expect_err("request binding panic faults the runtime");
    assert!(
        request_error
            .to_string()
            .contains("binding conversion panicked"),
        "{request_error}"
    );
    let shutdown_error = task
        .shutdown(Shutdown::Cancel)
        .await
        .expect_err("the owner observes the binding fault");
    assert_eq!(request_error.to_string(), shutdown_error.to_string());
    Ok(())
}
