use std::{convert::Infallible, future::Future, pin::Pin, sync::Arc, time::Duration};

use crate::runtime::{ActorId, EffectFuture, Envelope, EnvelopeHeader, Meta};

pub type SystemFuture<'a, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;
pub type SystemUnitFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Endpoint(pub String);

impl From<&str> for Endpoint {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SocketId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemIoError {
    pub message: String,
}

impl SystemIoError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

pub trait SystemBackend: Send + Sync + 'static {
    fn sleep(&self, duration: Duration) -> SystemUnitFuture<'_>;

    fn stdout_println(&self, line: String) -> SystemUnitFuture<'_>;

    fn socket_connect(&self, endpoint: Endpoint) -> SystemFuture<'_, SocketId, SystemIoError>;
}

#[derive(Default)]
pub struct TokioBackend;

impl SystemBackend for TokioBackend {
    fn sleep(&self, duration: Duration) -> SystemUnitFuture<'_> {
        Box::pin(async move {
            tokio::time::sleep(duration).await;
        })
    }

    fn stdout_println(&self, line: String) -> SystemUnitFuture<'_> {
        Box::pin(async move {
            println!("{line}");
        })
    }

    fn socket_connect(&self, endpoint: Endpoint) -> SystemFuture<'_, SocketId, SystemIoError> {
        Box::pin(async move {
            Err(SystemIoError::new(format!(
                "TokioBackend socket_connect not implemented for endpoint {}",
                endpoint.0
            )))
        })
    }
}

pub trait SystemEffect: Send + 'static {
    type Output: Send + 'static;
    type Error: Send + 'static;

    fn execute(
        self,
        backend: Arc<dyn SystemBackend>,
    ) -> SystemFuture<'static, Self::Output, Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sleep(pub Duration);

impl SystemEffect for Sleep {
    type Output = ();
    type Error = Infallible;

    fn execute(
        self,
        backend: Arc<dyn SystemBackend>,
    ) -> SystemFuture<'static, Self::Output, Self::Error> {
        Box::pin(async move {
            backend.sleep(self.0).await;
            Ok(())
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StdoutPrintln(pub String);

impl From<&str> for StdoutPrintln {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl SystemEffect for StdoutPrintln {
    type Output = ();
    type Error = Infallible;

    fn execute(
        self,
        backend: Arc<dyn SystemBackend>,
    ) -> SystemFuture<'static, Self::Output, Self::Error> {
        Box::pin(async move {
            backend.stdout_println(self.0).await;
            Ok(())
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SocketConnect(pub Endpoint);

impl SystemEffect for SocketConnect {
    type Output = SocketId;
    type Error = SystemIoError;

    fn execute(
        self,
        backend: Arc<dyn SystemBackend>,
    ) -> SystemFuture<'static, Self::Output, Self::Error> {
        Box::pin(async move { backend.socket_connect(self.0).await })
    }
}

pub trait ErasedComposedEffect: Send {
    fn run(self: Box<Self>, backend: Arc<dyn SystemBackend>) -> EffectFuture;
}

pub struct ComposedEffect<E, OkMap, ErrMap>
where
    E: SystemEffect,
    OkMap: FnOnce(E::Output) -> Vec<Envelope> + Send + 'static,
    ErrMap: FnOnce(E::Error) -> Vec<Envelope> + Send + 'static,
{
    effect: E,
    on_ok: OkMap,
    on_err: ErrMap,
}

impl<E, OkMap, ErrMap> ErasedComposedEffect for ComposedEffect<E, OkMap, ErrMap>
where
    E: SystemEffect,
    OkMap: FnOnce(E::Output) -> Vec<Envelope> + Send + 'static,
    ErrMap: FnOnce(E::Error) -> Vec<Envelope> + Send + 'static,
{
    fn run(self: Box<Self>, backend: Arc<dyn SystemBackend>) -> EffectFuture {
        let ComposedEffect {
            effect,
            on_ok,
            on_err,
        } = *self;
        Box::pin(async move {
            let envelopes = match effect.execute(backend).await {
                Ok(output) => on_ok(output),
                Err(err) => on_err(err),
            };
            Ok(envelopes)
        })
    }
}

pub fn composed<E, OkMap, ErrMap>(
    effect: E,
    on_ok: OkMap,
    on_err: ErrMap,
) -> ComposedEffect<E, OkMap, ErrMap>
where
    E: SystemEffect,
    OkMap: FnOnce(E::Output) -> Vec<Envelope> + Send + 'static,
    ErrMap: FnOnce(E::Error) -> Vec<Envelope> + Send + 'static,
{
    ComposedEffect {
        effect,
        on_ok,
        on_err,
    }
}

pub enum EffectRun {
    Future(EffectFuture),
    Composed(Vec<Box<dyn ErasedComposedEffect>>),
    Sequence(Vec<EffectRun>),
}

impl EffectRun {
    pub fn none() -> Self {
        Self::Sequence(Vec::new())
    }

    pub fn future(future: EffectFuture) -> Self {
        Self::Future(future)
    }

    pub fn user<F>(future: F) -> Self
    where
        F: Future<Output = Result<Vec<Envelope>, crate::runtime::EffectError>> + Send + 'static,
    {
        Self::Future(Box::pin(future))
    }

    pub fn system<E>(effect: E) -> Self
    where
        E: ErasedComposedEffect + 'static,
    {
        Self::Composed(vec![Box::new(effect)])
    }

    pub fn system_effect<E>(effect: E) -> SystemEffectBuilder<E>
    where
        E: SystemEffect,
    {
        SystemEffectBuilder { effect }
    }

    pub fn side_effect<E>(effect: E) -> Self
    where
        E: SystemEffect<Output = (), Error = Infallible>,
    {
        Self::system_effect(effect).ignore_output().into_run()
    }

    pub fn side_effect_future<F>(future: F) -> Self
    where
        F: Future<Output = ()> + Send + 'static,
    {
        Self::user(async move {
            future.await;
            Ok(Vec::new())
        })
    }

    pub fn and(self, next: Self) -> Self {
        match (self, next) {
            (Self::Sequence(mut left), Self::Sequence(right)) => {
                left.extend(right);
                Self::Sequence(left)
            }
            (Self::Sequence(mut left), right) => {
                left.push(right);
                Self::Sequence(left)
            }
            (left, Self::Sequence(mut right)) => {
                let mut out = Vec::with_capacity(1 + right.len());
                out.push(left);
                out.append(&mut right);
                Self::Sequence(out)
            }
            (left, right) => Self::Sequence(vec![left, right]),
        }
    }

    pub fn and_user<F>(self, future: F) -> Self
    where
        F: Future<Output = Result<Vec<Envelope>, crate::runtime::EffectError>> + Send + 'static,
    {
        self.and(Self::user(future))
    }

    pub fn and_system<E>(self, effect: E) -> Self
    where
        E: ErasedComposedEffect + 'static,
    {
        self.and(Self::system(effect))
    }

    pub fn and_side_effect<E>(self, effect: E) -> Self
    where
        E: SystemEffect<Output = (), Error = Infallible>,
    {
        self.and(Self::side_effect(effect))
    }

    pub fn and_side_effect_future<F>(self, future: F) -> Self
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.and(Self::side_effect_future(future))
    }
}

pub struct SystemEffectBuilder<E>
where
    E: SystemEffect,
{
    effect: E,
}

impl<E> SystemEffectBuilder<E>
where
    E: SystemEffect,
{
    pub fn on_ok<OkMap>(self, on_ok: OkMap) -> SystemEffectOkBuilder<E, OkMap>
    where
        OkMap: FnOnce(E::Output) -> Vec<Envelope> + Send + 'static,
    {
        SystemEffectOkBuilder {
            effect: self.effect,
            on_ok,
        }
    }
}

pub struct SystemEffectOkBuilder<E, OkMap>
where
    E: SystemEffect,
    OkMap: FnOnce(E::Output) -> Vec<Envelope> + Send + 'static,
{
    effect: E,
    on_ok: OkMap,
}

impl<E, OkMap> SystemEffectOkBuilder<E, OkMap>
where
    E: SystemEffect<Error = Infallible>,
    OkMap: FnOnce(E::Output) -> Vec<Envelope> + Send + 'static,
{
    pub fn into_run(self) -> EffectRun {
        EffectRun::system(composed(self.effect, self.on_ok, infallible_to_envelopes))
    }
}

impl<E> SystemEffectBuilder<E>
where
    E: SystemEffect<Output = (), Error = Infallible>,
{
    pub fn ignore_output(self) -> SystemEffectOkBuilder<E, fn(()) -> Vec<Envelope>> {
        fn empty(_: ()) -> Vec<Envelope> {
            Vec::new()
        }

        SystemEffectOkBuilder {
            effect: self.effect,
            on_ok: empty,
        }
    }
}

impl<E, OkMap> SystemEffectOkBuilder<E, OkMap>
where
    E: SystemEffect,
    OkMap: FnOnce(E::Output) -> Vec<Envelope> + Send + 'static,
{
    pub fn on_err<ErrMap>(self, on_err: ErrMap) -> EffectRun
    where
        ErrMap: FnOnce(E::Error) -> Vec<Envelope> + Send + 'static,
    {
        EffectRun::system(composed(self.effect, self.on_ok, on_err))
    }
}

pub fn materialize_effect_run(run: EffectRun, backend: Arc<dyn SystemBackend>) -> EffectFuture {
    match run {
        EffectRun::Future(fut) => fut,
        EffectRun::Composed(effects) => Box::pin(async move {
            let mut envelopes = Vec::new();
            for effect in effects {
                envelopes.extend(effect.run(Arc::clone(&backend)).await?);
            }
            Ok(envelopes)
        }),
        EffectRun::Sequence(runs) => Box::pin(async move {
            let mut envelopes = Vec::new();
            for run in runs {
                envelopes.extend(materialize_effect_run(run, Arc::clone(&backend)).await?);
            }
            Ok(envelopes)
        }),
    }
}

pub fn infallible_to_envelopes(never: Infallible) -> Vec<Envelope> {
    match never {}
}

pub fn runtime_header_for_system_effect(origin: ActorId, meta: Meta) -> EnvelopeHeader {
    EnvelopeHeader {
        to: origin,
        msg_type: std::any::TypeId::of::<()>(),
        meta,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::sync::Mutex;

    use super::{
        EffectRun, Endpoint, Sleep, SocketConnect, SocketId, StdoutPrintln, SystemBackend,
        SystemFuture, SystemIoError, SystemUnitFuture, materialize_effect_run,
    };
    use crate::runtime::{ActorId, Envelope};

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum TestMsg {
        Slept,
        Connected(SocketId),
        ConnectFailed(String),
        UserEffectDone,
    }

    struct MockBackend {
        slept: Mutex<Vec<Duration>>,
        printed: Mutex<Vec<String>>,
        connect_result: Mutex<Result<SocketId, SystemIoError>>,
    }

    impl MockBackend {
        fn new(connect_result: Result<SocketId, SystemIoError>) -> Self {
            Self {
                slept: Mutex::new(Vec::new()),
                printed: Mutex::new(Vec::new()),
                connect_result: Mutex::new(connect_result),
            }
        }
    }

    impl SystemBackend for MockBackend {
        fn sleep(&self, duration: Duration) -> SystemUnitFuture<'_> {
            Box::pin(async move {
                self.slept.lock().await.push(duration);
            })
        }

        fn stdout_println(&self, line: String) -> SystemUnitFuture<'_> {
            Box::pin(async move {
                self.printed.lock().await.push(line);
            })
        }

        fn socket_connect(&self, _endpoint: Endpoint) -> SystemFuture<'_, SocketId, SystemIoError> {
            Box::pin(async move { self.connect_result.lock().await.clone() })
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn infallible_sleep_effect_maps_only_success_path() {
        let backend = std::sync::Arc::new(MockBackend::new(Ok(SocketId(9))));
        let run = EffectRun::system_effect(Sleep(Duration::from_millis(10)))
            .on_ok(|_| vec![Envelope::new(ActorId(1), TestMsg::Slept)])
            .into_run();

        let envelopes = materialize_effect_run(run, backend.clone())
            .await
            .expect("composed effect should resolve");

        let msg = extract_msg::<TestMsg>(envelopes.into_iter().next().expect("one envelope"));
        assert_eq!(msg, TestMsg::Slept);
        assert_eq!(
            backend.slept.lock().await.as_slice(),
            &[Duration::from_millis(10)]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fallible_socket_connect_effect_maps_error_path() {
        let backend = std::sync::Arc::new(MockBackend::new(Err(SystemIoError::new("no route"))));
        let run = EffectRun::system_effect(SocketConnect(Endpoint::from("127.0.0.1:9000")))
            .on_ok(|socket| vec![Envelope::new(ActorId(1), TestMsg::Connected(socket))])
            .on_err(|err| {
                vec![Envelope::new(
                    ActorId(1),
                    TestMsg::ConnectFailed(err.message),
                )]
            });

        let envelopes = materialize_effect_run(run, backend)
            .await
            .expect("composed effect should resolve");

        let msg = extract_msg::<TestMsg>(envelopes.into_iter().next().expect("one envelope"));
        assert_eq!(msg, TestMsg::ConnectFailed("no route".to_string()));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn helpers_combine_user_and_system_effects() {
        let backend = std::sync::Arc::new(MockBackend::new(Ok(SocketId(1))));
        let run =
            EffectRun::user(async { Ok(vec![Envelope::new(ActorId(1), TestMsg::UserEffectDone)]) })
                .and(
                    EffectRun::system_effect(Sleep(Duration::from_millis(1)))
                        .on_ok(|_| vec![Envelope::new(ActorId(1), TestMsg::Slept)])
                        .into_run(),
                );

        let envelopes = materialize_effect_run(run, backend)
            .await
            .expect("combined run should succeed");

        assert_eq!(envelopes.len(), 2);
        let mut iter = envelopes.into_iter();
        assert_eq!(
            extract_msg::<TestMsg>(iter.next().expect("first envelope")),
            TestMsg::UserEffectDone
        );
        assert_eq!(
            extract_msg::<TestMsg>(iter.next().expect("second envelope")),
            TestMsg::Slept
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn side_effect_run_emits_no_messages() {
        let backend = std::sync::Arc::new(MockBackend::new(Ok(SocketId(1))));
        let run = EffectRun::side_effect(StdoutPrintln::from("hello"));

        let envelopes = materialize_effect_run(run, backend.clone())
            .await
            .expect("side effect should succeed");

        assert!(envelopes.is_empty());
        assert_eq!(
            backend.printed.lock().await.as_slice(),
            &["hello".to_string()]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn and_side_effect_chains_with_user_effect() {
        let backend = std::sync::Arc::new(MockBackend::new(Ok(SocketId(1))));
        let run =
            EffectRun::user(async { Ok(vec![Envelope::new(ActorId(1), TestMsg::UserEffectDone)]) })
                .and_side_effect(StdoutPrintln::from("done"));

        let envelopes = materialize_effect_run(run, backend.clone())
            .await
            .expect("combined run should succeed");

        assert_eq!(envelopes.len(), 1);
        assert_eq!(
            extract_msg::<TestMsg>(envelopes.into_iter().next().expect("one envelope")),
            TestMsg::UserEffectDone
        );
        assert_eq!(
            backend.printed.lock().await.as_slice(),
            &["done".to_string()]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn side_effect_future_accepts_direct_async_block() {
        let backend = std::sync::Arc::new(MockBackend::new(Ok(SocketId(1))));
        let marker = std::sync::Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let marker_clone = std::sync::Arc::clone(&marker);

        let run = EffectRun::side_effect_future(async move {
            marker_clone.lock().await.push("ran");
        });

        let envelopes = materialize_effect_run(run, backend)
            .await
            .expect("side-effect future should succeed");

        assert!(envelopes.is_empty());
        assert_eq!(marker.lock().await.as_slice(), &["ran"]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn and_side_effect_future_chains_with_user_effect() {
        let backend = std::sync::Arc::new(MockBackend::new(Ok(SocketId(1))));
        let marker = std::sync::Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let marker_clone = std::sync::Arc::clone(&marker);

        let run =
            EffectRun::user(async { Ok(vec![Envelope::new(ActorId(1), TestMsg::UserEffectDone)]) })
                .and_side_effect_future(async move {
                    marker_clone.lock().await.push("ran");
                });

        let envelopes = materialize_effect_run(run, backend)
            .await
            .expect("combined run should succeed");

        assert_eq!(envelopes.len(), 1);
        assert_eq!(
            extract_msg::<TestMsg>(envelopes.into_iter().next().expect("one envelope")),
            TestMsg::UserEffectDone
        );
        assert_eq!(marker.lock().await.as_slice(), &["ran"]);
    }

    fn extract_msg<M: Send + 'static>(env: Envelope) -> M {
        *env.body
            .downcast::<M>()
            .expect("envelope should contain expected message type")
    }
}
