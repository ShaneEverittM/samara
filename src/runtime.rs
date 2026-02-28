use std::{
    any::{Any, TypeId},
    collections::HashMap,
    future::Future,
    marker::PhantomData,
    pin::Pin,
    sync::Arc,
};

use tokio::{
    sync::mpsc,
    task::{JoinError, JoinSet},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ActorId(pub u64);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Meta {
    pub correlation_id: Option<u64>,
    pub causation_id: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvelopeHeader {
    pub to: ActorId,
    pub msg_type: TypeId,
    pub meta: Meta,
}

pub struct Envelope {
    pub to: ActorId,
    pub msg_type: TypeId,
    pub body: Box<dyn Any + Send>,
    pub meta: Meta,
}

impl Envelope {
    pub fn new<M>(to: ActorId, msg: M) -> Self
    where
        M: Send + 'static,
    {
        Self::with_meta(to, msg, Meta::default())
    }

    pub fn with_meta<M>(to: ActorId, msg: M, meta: Meta) -> Self
    where
        M: Send + 'static,
    {
        Self {
            to,
            msg_type: TypeId::of::<M>(),
            body: Box::new(msg),
            meta,
        }
    }

    pub fn header(&self) -> EnvelopeHeader {
        EnvelopeHeader {
            to: self.to,
            msg_type: self.msg_type,
            meta: self.meta.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendError {
    MailboxClosed,
}

pub struct Addr<M> {
    id: ActorId,
    tx: mpsc::Sender<Envelope>,
    marker: PhantomData<fn(M)>,
}

impl<M> Clone for Addr<M> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            tx: self.tx.clone(),
            marker: PhantomData,
        }
    }
}

impl<M> Addr<M> {
    pub fn actor_id(&self) -> ActorId {
        self.id
    }
}

impl<M> Addr<M>
where
    M: Send + 'static,
{
    pub async fn send(&self, msg: M) -> Result<(), SendError> {
        self.send_with_meta(msg, Meta::default()).await
    }

    pub async fn send_with_meta(&self, msg: M, meta: Meta) -> Result<(), SendError> {
        let env = Envelope::with_meta(self.id, msg, meta);
        self.tx.send(env).await.map_err(|_| SendError::MailboxClosed)
    }
}

#[derive(Clone, Debug)]
pub struct IssuedCmd<C> {
    pub origin: ActorId,
    pub meta: Meta,
    pub cmd: C,
}

pub trait Actor: Send + 'static {
    type Msg: Send + 'static;
    type Cmd: Send + 'static;

    fn on_msg(&mut self, msg: Self::Msg) -> Vec<Self::Cmd>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchErrorKind {
    TypeMismatch,
    DowncastFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchError {
    pub header: EnvelopeHeader,
    pub expected_type: TypeId,
    pub kind: DispatchErrorKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeadLetterReason {
    UnknownTarget,
    TypeMismatch,
    DowncastFailed,
    EffectHandlerFailed(String),
    EffectJoinFailed(String),
    MailboxClosed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeadLetter {
    pub header: EnvelopeHeader,
    pub reason: DeadLetterReason,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectError {
    pub origin: ActorId,
    pub meta: Meta,
    pub message: String,
}

pub type EffectFuture = Pin<Box<dyn Future<Output = Result<Vec<Envelope>, EffectError>> + Send>>;
pub type EffectHandler<C> = Arc<dyn Fn(IssuedCmd<C>) -> EffectFuture + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegisterError {
    DuplicateActorId(ActorId),
}

trait ErasedActor<C>: Send {
    fn dispatch(&mut self, env: Envelope) -> Result<Vec<IssuedCmd<C>>, DispatchError>;
    fn as_any(&self) -> &dyn Any;
}

struct ActorCell<A, C> {
    id: ActorId,
    actor: A,
    marker: PhantomData<C>,
}

impl<A, C> ErasedActor<C> for ActorCell<A, C>
where
    A: Actor<Cmd = C>,
    C: Send + 'static,
{
    fn dispatch(&mut self, env: Envelope) -> Result<Vec<IssuedCmd<C>>, DispatchError> {
        let header = env.header();
        let expected_type = TypeId::of::<A::Msg>();
        if header.msg_type != expected_type {
            return Err(DispatchError {
                header,
                expected_type,
                kind: DispatchErrorKind::TypeMismatch,
            });
        }

        let body = env.body.downcast::<A::Msg>().map_err(|_| DispatchError {
            header: header.clone(),
            expected_type,
            kind: DispatchErrorKind::DowncastFailed,
        })?;

        let cmds = self
            .actor
            .on_msg(*body)
            .into_iter()
            .map(|cmd| IssuedCmd {
                origin: self.id,
                meta: header.meta.clone(),
                cmd,
            })
            .collect();

        Ok(cmds)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct Runtime<C>
where
    C: Send + 'static,
{
    tx: mpsc::Sender<Envelope>,
    rx: mpsc::Receiver<Envelope>,
    registry: HashMap<ActorId, Box<dyn ErasedActor<C>>>,
    effect_handler: EffectHandler<C>,
    dead_letters: Vec<DeadLetter>,
}

impl<C> Runtime<C>
where
    C: Send + 'static,
{
    pub fn new(mailbox_capacity: usize, effect_handler: EffectHandler<C>) -> Self {
        let (tx, rx) = mpsc::channel(mailbox_capacity.max(1));
        Self {
            tx,
            rx,
            registry: HashMap::new(),
            effect_handler,
            dead_letters: Vec::new(),
        }
    }

    pub fn register_actor<A>(&mut self, id: ActorId, actor: A) -> Result<Addr<A::Msg>, RegisterError>
    where
        A: Actor<Cmd = C> + 'static,
    {
        if self.registry.contains_key(&id) {
            return Err(RegisterError::DuplicateActorId(id));
        }

        self.registry.insert(
            id,
            Box::new(ActorCell::<A, C> {
                id,
                actor,
                marker: PhantomData,
            }),
        );

        Ok(Addr {
            id,
            tx: self.tx.clone(),
            marker: PhantomData,
        })
    }

    pub async fn send_envelope(&self, envelope: Envelope) -> Result<(), SendError> {
        self.tx
            .send(envelope)
            .await
            .map_err(|_| SendError::MailboxClosed)
    }

    pub fn actor<A>(&self, id: ActorId) -> Option<&A>
    where
        A: Actor<Cmd = C> + 'static,
    {
        self.registry
            .get(&id)?
            .as_any()
            .downcast_ref::<ActorCell<A, C>>()
            .map(|cell| &cell.actor)
    }

    pub fn dead_letters(&self) -> &[DeadLetter] {
        &self.dead_letters
    }

    pub async fn run_for(&mut self, duration: Duration) {
        let deadline = Instant::now() + duration;
        let mut effects = JoinSet::<Result<Vec<Envelope>, EffectError>>::new();

        loop {
            if Instant::now() >= deadline {
                break;
            }

            let sleep = tokio::time::sleep_until(deadline);
            tokio::pin!(sleep);

            tokio::select! {
                _ = &mut sleep => {
                    break;
                }
                maybe_env = self.rx.recv() => {
                    let Some(env) = maybe_env else {
                        break;
                    };
                    self.dispatch_envelope(env, &mut effects);
                }
                maybe_joined = effects.join_next(), if !effects.is_empty() => {
                    self.handle_effect_join(maybe_joined).await;
                }
            }
        }

        effects.abort_all();
        while let Some(joined) = effects.join_next().await {
            if let Err(err) = joined {
                self.dead_letters.push(DeadLetter {
                    header: EnvelopeHeader {
                        to: ActorId(0),
                        msg_type: TypeId::of::<()>(),
                        meta: Meta::default(),
                    },
                    reason: DeadLetterReason::EffectJoinFailed(err.to_string()),
                });
            }
        }
    }

    fn dispatch_envelope(
        &mut self,
        env: Envelope,
        effects: &mut JoinSet<Result<Vec<Envelope>, EffectError>>,
    ) {
        let header = env.header();
        let Some(actor) = self.registry.get_mut(&header.to) else {
            self.dead_letters.push(DeadLetter {
                header,
                reason: DeadLetterReason::UnknownTarget,
            });
            return;
        };

        match actor.dispatch(env) {
            Ok(cmds) => {
                for issued in cmds {
                    let handler = Arc::clone(&self.effect_handler);
                    effects.spawn(async move { (handler)(issued).await });
                }
            }
            Err(err) => {
                let reason = match err.kind {
                    DispatchErrorKind::TypeMismatch => DeadLetterReason::TypeMismatch,
                    DispatchErrorKind::DowncastFailed => DeadLetterReason::DowncastFailed,
                };
                self.dead_letters.push(DeadLetter {
                    header: err.header,
                    reason,
                });
            }
        }
    }

    async fn handle_effect_join(
        &mut self,
        maybe_joined: Option<Result<Result<Vec<Envelope>, EffectError>, JoinError>>,
    ) {
        let Some(joined) = maybe_joined else {
            return;
        };

        match joined {
            Ok(Ok(envelopes)) => {
                for env in envelopes {
                    let header = env.header();
                    if self.tx.send(env).await.is_err() {
                        self.dead_letters.push(DeadLetter {
                            header,
                            reason: DeadLetterReason::MailboxClosed,
                        });
                    }
                }
            }
            Ok(Err(err)) => {
                self.dead_letters.push(DeadLetter {
                    header: EnvelopeHeader {
                        to: err.origin,
                        msg_type: TypeId::of::<()>(),
                        meta: err.meta,
                    },
                    reason: DeadLetterReason::EffectHandlerFailed(err.message),
                });
            }
            Err(err) => {
                self.dead_letters.push(DeadLetter {
                    header: EnvelopeHeader {
                        to: ActorId(0),
                        msg_type: TypeId::of::<()>(),
                        meta: Meta::default(),
                    },
                    reason: DeadLetterReason::EffectJoinFailed(err.to_string()),
                });
            }
        }
    }
}
