use std::{
    convert::Infallible,
    future::Future,
    pin::Pin,
    sync::Arc,
    time::Duration,
};

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

    fn socket_connect(
        &self,
        endpoint: Endpoint,
    ) -> SystemFuture<'_, SocketId, SystemIoError>;
}

#[derive(Default)]
pub struct TokioBackend;

impl SystemBackend for TokioBackend {
    fn sleep(&self, duration: Duration) -> SystemUnitFuture<'_> {
        Box::pin(async move {
            tokio::time::sleep(duration).await;
        })
    }

    fn socket_connect(
        &self,
        endpoint: Endpoint,
    ) -> SystemFuture<'_, SocketId, SystemIoError> {
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

    fn execute(self, backend: Arc<dyn SystemBackend>) -> SystemFuture<'static, Self::Output, Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sleep(pub Duration);

impl SystemEffect for Sleep {
    type Output = ();
    type Error = Infallible;

    fn execute(self, backend: Arc<dyn SystemBackend>) -> SystemFuture<'static, Self::Output, Self::Error> {
        Box::pin(async move {
            backend.sleep(self.0).await;
            Ok(())
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SocketConnect(pub Endpoint);

impl SystemEffect for SocketConnect {
    type Output = SocketId;
    type Error = SystemIoError;

    fn execute(self, backend: Arc<dyn SystemBackend>) -> SystemFuture<'static, Self::Output, Self::Error> {
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
        composed, infallible_to_envelopes, materialize_effect_run, EffectRun, Endpoint, Sleep,
        SocketConnect, SocketId, SystemBackend, SystemFuture, SystemIoError, SystemUnitFuture,
    };
    use crate::runtime::{ActorId, Envelope};

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum TestMsg {
        Slept,
        Connected(SocketId),
        ConnectFailed(String),
    }

    struct MockBackend {
        slept: Mutex<Vec<Duration>>,
        connect_result: Mutex<Result<SocketId, SystemIoError>>,
    }

    impl MockBackend {
        fn new(connect_result: Result<SocketId, SystemIoError>) -> Self {
            Self {
                slept: Mutex::new(Vec::new()),
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

        fn socket_connect(
            &self,
            _endpoint: Endpoint,
        ) -> SystemFuture<'_, SocketId, SystemIoError> {
            Box::pin(async move { self.connect_result.lock().await.clone() })
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn infallible_sleep_effect_maps_only_success_path() {
        let backend = std::sync::Arc::new(MockBackend::new(Ok(SocketId(9))));
        let effect = composed(
            Sleep(Duration::from_millis(10)),
            |_| vec![Envelope::new(ActorId(1), TestMsg::Slept)],
            infallible_to_envelopes,
        );

        let envelopes = materialize_effect_run(
            EffectRun::Composed(vec![Box::new(effect)]),
            backend.clone(),
        )
        .await
        .expect("composed effect should resolve");

        let msg = extract_msg::<TestMsg>(envelopes.into_iter().next().expect("one envelope"));
        assert_eq!(msg, TestMsg::Slept);
        assert_eq!(backend.slept.lock().await.as_slice(), &[Duration::from_millis(10)]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fallible_socket_connect_effect_maps_error_path() {
        let backend = std::sync::Arc::new(MockBackend::new(Err(SystemIoError::new("no route"))));
        let effect = composed(
            SocketConnect(Endpoint::from("127.0.0.1:9000")),
            |socket| vec![Envelope::new(ActorId(1), TestMsg::Connected(socket))],
            |err| vec![Envelope::new(ActorId(1), TestMsg::ConnectFailed(err.message))],
        );

        let envelopes = materialize_effect_run(
            EffectRun::Composed(vec![Box::new(effect)]),
            backend,
        )
        .await
        .expect("composed effect should resolve");

        let msg = extract_msg::<TestMsg>(envelopes.into_iter().next().expect("one envelope"));
        assert_eq!(msg, TestMsg::ConnectFailed("no route".to_string()));
    }

    fn extract_msg<M: Send + 'static>(env: Envelope) -> M {
        *env.body
            .downcast::<M>()
            .expect("envelope should contain expected message type")
    }
}
