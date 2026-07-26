use std::{
    convert::Infallible,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use samara::prelude::*;

protocol! {
    type HostProtocol => enum HostProtocolMessage {
        Set(u64),
        RecordIngress(u64),
        Read -> u64,
        Delayed(u64) -> u64,
        Held(u64) -> u64,
        Never -> (),
        Crash -> (),
    }
}

struct CountedSet {
    value: u64,
    conversions: Arc<AtomicUsize>,
}

impl Notification<HostProtocol> for CountedSet {
    fn into_message(self) -> HostProtocolMessage {
        self.conversions.fetch_add(1, Ordering::SeqCst);
        HostProtocolMessage::Set(self.value)
    }
}

struct PanickingRequest;

impl Request<HostProtocol> for PanickingRequest {
    type Reply = ();

    fn into_message(self, _reply_to: ReplyTo<Self::Reply>) -> HostProtocolMessage {
        panic!("request conversion panic")
    }
}

struct PanickingNotification;

impl Notification<HostProtocol> for PanickingNotification {
    fn into_message(self) -> HostProtocolMessage {
        panic!("notification conversion panic")
    }
}

struct BindingProtocol;

enum BindingProtocolMessage {
    Notification,
    Request(RequestInvocation<BindingProtocol, BindingRequest>),
}

impl Protocol for BindingProtocol {
    type Message = BindingProtocolMessage;
}

struct BindingNotification;

impl Notification<BindingProtocol> for BindingNotification {
    fn into_message(self) -> BindingProtocolMessage {
        BindingProtocolMessage::Notification
    }
}

struct BindingRequest;

impl Request<BindingProtocol> for BindingRequest {
    type Reply = ();

    fn into_message(self, reply_to: ReplyTo<Self::Reply>) -> BindingProtocolMessage {
        BindingProtocolMessage::Request(RequestInvocation::new(self, reply_to))
    }
}

struct PanickingBindingProviderMessage;

impl From<BindingProtocolMessage> for PanickingBindingProviderMessage {
    fn from(message: BindingProtocolMessage) -> Self {
        if let BindingProtocolMessage::Request(request) = message {
            let _ = request.reply_to;
        }
        panic!("provider binding conversion panic")
    }
}

struct PanickingBindingProvider;

impl Component for PanickingBindingProvider {
    type Model = ();
    type Message = PanickingBindingProviderMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(())
    }

    fn update(&self, _model: &mut Self::Model, _message: Self::Message) -> Command<Self::Message> {
        Command::none()
    }
}

enum ProviderMessage {
    Protocol(HostProtocolMessage),
    RecordIngressDirect(u64),
    CompleteDelayed { reply_to: ReplyTo<u64>, value: u64 },
    Signalled,
}

impl From<HostProtocolMessage> for ProviderMessage {
    fn from(message: HostProtocolMessage) -> Self {
        Self::Protocol(message)
    }
}

struct Provider {
    ingress_observed: EffectCapability<IngressObserved>,
    request_observed: EffectCapability<RequestObserved>,
    await_release: EffectCapability<AwaitRelease>,
}

impl Component for Provider {
    type Model = u64;
    type Message = ProviderMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(0)
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            ProviderMessage::Protocol(HostProtocolMessage::Set(value)) => {
                *model = value;
                Command::none()
            }
            ProviderMessage::Protocol(HostProtocolMessage::RecordIngress(value))
            | ProviderMessage::RecordIngressDirect(value) => {
                Command::effect_discarding_outcome(&self.ingress_observed, IngressObserved(value))
            }
            ProviderMessage::Protocol(HostProtocolMessage::Read(request)) => {
                Command::reply(request.reply_to, *model)
            }
            ProviderMessage::Protocol(HostProtocolMessage::Delayed(request)) => {
                let delay = if request.request.0 == 11 {
                    Duration::from_millis(50)
                } else {
                    Duration::from_millis(10)
                };
                Command::batch([
                    Command::effect_with(&self.request_observed, RequestObserved, |_| {
                        ProviderMessage::Signalled
                    }),
                    Command::after(
                        delay,
                        ProviderMessage::CompleteDelayed {
                            reply_to: request.reply_to,
                            value: request.request.0,
                        },
                    ),
                ])
            }
            ProviderMessage::Protocol(HostProtocolMessage::Held(request)) => {
                let reply_to = request.reply_to;
                Command::effect_with(
                    &self.await_release,
                    AwaitRelease(request.request.0),
                    move |outcome| match outcome {
                        EffectOutcome::Succeeded(value) => {
                            ProviderMessage::CompleteDelayed { reply_to, value }
                        }
                        EffectOutcome::Failed(never) => match never {},
                        EffectOutcome::Cancelled(_) => ProviderMessage::Signalled,
                    },
                )
            }
            ProviderMessage::Protocol(HostProtocolMessage::Never(_request)) => {
                Command::effect_with(&self.request_observed, RequestObserved, |_| {
                    ProviderMessage::Signalled
                })
            }
            ProviderMessage::Protocol(HostProtocolMessage::Crash(_request)) => {
                panic!("provider crashed while handling an external request")
            }
            ProviderMessage::CompleteDelayed { reply_to, value } => Command::reply(reply_to, value),
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

fn program() -> (Program, Port<HostProtocol>) {
    let mut builder = Program::builder();
    let port = builder.port(PortId::new("host"));
    let ingress_observed = builder.effect::<IngressObserved>();
    let request_observed = builder.effect::<RequestObserved>();
    let await_release = builder.effect::<AwaitRelease>();
    let provider = builder.component(
        ComponentId::new("provider"),
        Provider {
            ingress_observed,
            request_observed,
            await_release,
        },
    );
    builder.bind_port(&port, &provider);
    (builder.build().expect("valid Program"), port)
}

fn panicking_binding_program() -> (Program, Port<BindingProtocol>) {
    let mut builder = Program::builder();
    let port = builder.port(PortId::new("panicking-binding"));
    let provider = builder.component(
        ComponentId::new("panicking-binding-provider"),
        PanickingBindingProvider,
    );
    builder.bind_port(&port, &provider);
    (builder.build().expect("valid binding Program"), port)
}

fn runtime_with_observer(
    program: Program,
) -> (LiveRuntime, tokio::sync::mpsc::UnboundedReceiver<()>) {
    let (observed, observations) = tokio::sync::mpsc::unbounded_channel();
    let (ingress_observed, _ingress_observations) = tokio::sync::mpsc::unbounded_channel();
    let runtime = LiveRuntime::builder(program)
        .bind_effect::<RequestObserved, _>(ObservationDriver { observed })
        .bind_effect::<AwaitRelease, _>(ImmediateReleaseDriver)
        .bind_effect::<IngressObserved, _>(IngressObserver {
            observed: ingress_observed,
        })
        .build()
        .expect("valid live bindings");
    (runtime, observations)
}

fn runtime_with_default_effects(program: Program) -> LiveRuntime {
    let (request_observed, _request_observations) = tokio::sync::mpsc::unbounded_channel();
    let (ingress_observed, _ingress_observations) = tokio::sync::mpsc::unbounded_channel();
    LiveRuntime::builder(program)
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
    let foreign_port = foreign_builder.port::<HostProtocol>(PortId::new("host"));

    let error = runtime
        .port_handle(&foreign_port)
        .expect_err("a foreign Port cannot become a live capability");
    assert!(error.to_string().contains("not part of this Program"));
}

#[tokio::test]
async fn live_port_handle_notifies_and_returns_typed_request_replies() -> Result<(), RuntimeError> {
    let (program, port) = program();
    let runtime = runtime_with_default_effects(program);
    let host = runtime.port_handle(&port)?;
    let task = runtime.spawn();

    let notifications = host.clone();
    notifications.notify(Set(41)).await?;
    assert_eq!(host.request(Read).await?, 41);

    assert!(task.shutdown(Shutdown::Drain).await?.is_clean());
    let conversions = Arc::new(AtomicUsize::new(0));
    assert!(
        host.notify(CountedSet {
            value: 42,
            conversions: conversions.clone(),
        })
        .await
        .is_err()
    );
    assert_eq!(conversions.load(Ordering::SeqCst), 0);
    assert!(host.request(Read).await.is_err());
    Ok(())
}

#[tokio::test]
async fn cloned_port_handles_deliver_each_notification_exactly_once() -> Result<(), RuntimeError> {
    let mut builder = Program::builder();
    let port = builder.port::<HostProtocol>(PortId::new("cloned-host"));
    let ingress_observed = builder.effect::<IngressObserved>();
    let request_observed = builder.effect::<RequestObserved>();
    let await_release = builder.effect::<AwaitRelease>();
    let provider = builder.component(
        ComponentId::new("cloned-provider"),
        Provider {
            ingress_observed,
            request_observed,
            await_release,
        },
    );
    builder.bind_port(&port, &provider);
    let (observed, mut observations) = tokio::sync::mpsc::unbounded_channel();
    let (request_observed, _request_observations) = tokio::sync::mpsc::unbounded_channel();
    let runtime = LiveRuntime::builder(builder.build().expect("valid cloned-handle Program"))
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
        sends.spawn(async move { host.notify(RecordIngress(value)).await });
    }
    while let Some(result) = sends.join_next().await {
        result.expect("notification task did not panic")?;
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
    let (left, right) = tokio::join!(host.request(Delayed(11)), host.request(Delayed(29)));
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

    let request = tokio::spawn(async move { host.request(Delayed(73)).await });
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

    let request = tokio::spawn(async move { host.request(Never).await });
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

    let request = host.request(Never);
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
    let runtime = LiveRuntime::builder(program)
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

    let request = tokio::spawn(async move { host.request(Held(5)).await });
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

    let request = tokio::spawn(async move { host.request(Never).await });
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
    let port = builder.port::<HostProtocol>(PortId::new("race-host"));
    let ingress_observed = builder.effect::<IngressObserved>();
    let request_observed = builder.effect::<RequestObserved>();
    let await_release = builder.effect::<AwaitRelease>();
    let provider = builder.component(
        ComponentId::new("race-provider"),
        Provider {
            ingress_observed,
            request_observed,
            await_release,
        },
    );
    builder.bind_port(&port, &provider);
    let (observed, mut observations) = tokio::sync::mpsc::unbounded_channel();
    let (request_observed, _request_observations) = tokio::sync::mpsc::unbounded_channel();
    let runtime = LiveRuntime::builder(builder.build().expect("valid race Program"))
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
            protocol.notify(RecordIngress(2)).await
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
        .request(Crash)
        .await
        .expect_err("the provider transition faults the runtime");
    assert!(request_error.to_string().contains("transition panicked"));
    let later_error = host
        .notify(Set(1))
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

    let request_error = host
        .request(PanickingRequest)
        .await
        .expect_err("conversion panic faults the runtime");
    assert!(
        request_error
            .to_string()
            .contains("external Request protocol conversion panicked")
    );

    let shutdown_error = task
        .shutdown(Shutdown::Cancel)
        .await
        .expect_err("the owner observes the conversion fault");
    assert_eq!(request_error.to_string(), shutdown_error.to_string());
    Ok(())
}

#[tokio::test]
async fn notification_conversion_panics_are_contained_as_runtime_faults() -> Result<(), RuntimeError>
{
    let (program, port) = program();
    let runtime = runtime_with_default_effects(program);
    let host = runtime.port_handle(&port)?;
    let task = runtime.spawn();

    host.notify(PanickingNotification).await?;
    let error = task
        .shutdown(Shutdown::Drain)
        .await
        .expect_err("notification conversion panic faults the runtime");
    assert!(
        error
            .to_string()
            .contains("external Notification protocol conversion panicked")
    );
    Ok(())
}

#[tokio::test]
async fn notification_binding_panics_are_contained_as_runtime_faults() -> Result<(), RuntimeError> {
    let (program, port) = panicking_binding_program();
    let runtime = LiveRuntime::builder(program).build()?;
    let host = runtime.port_handle(&port)?;
    let task = runtime.spawn();

    host.notify(BindingNotification).await?;
    let error = task
        .shutdown(Shutdown::Drain)
        .await
        .expect_err("notification binding panic faults the runtime");
    assert!(
        error
            .to_string()
            .contains("external Notification binding conversion panicked")
    );
    Ok(())
}

#[tokio::test]
async fn request_binding_panics_are_contained_as_runtime_faults() -> Result<(), RuntimeError> {
    let (program, port) = panicking_binding_program();
    let runtime = LiveRuntime::builder(program).build()?;
    let host = runtime.port_handle(&port)?;
    let task = runtime.spawn();

    let request_error = host
        .request(BindingRequest)
        .await
        .expect_err("request binding panic faults the runtime");
    assert!(
        request_error
            .to_string()
            .contains("external Request binding conversion panicked")
    );
    let shutdown_error = task
        .shutdown(Shutdown::Cancel)
        .await
        .expect_err("the owner observes the binding fault");
    assert_eq!(request_error.to_string(), shutdown_error.to_string());
    Ok(())
}
