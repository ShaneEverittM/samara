use std::{
    any::{Any, TypeId},
    collections::HashMap,
    future::Future,
    marker::PhantomData,
    pin::Pin,
    sync::{Arc, Mutex},
};

use tokio::{
    sync::mpsc,
    task::JoinSet,
    time::{Duration, Instant},
};

use crate::system_effects::{materialize_effect_run, EffectRun, SystemBackend};

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
    type Driver: EffectDriver<Self::Cmd>;
    type DriverContext;

    fn update(&mut self, msg: Self::Msg) -> Vec<Self::Cmd>;

    fn effect_driver(context: Self::DriverContext) -> Self::Driver
    where
        Self: Sized;
}

pub trait EffectDriver<C>: Send + Sync + 'static {
    fn run(&self, issued: IssuedCmd<C>) -> EffectRun;
}

impl<C, F> EffectDriver<C> for F
where
    C: Send + 'static,
    F: Fn(IssuedCmd<C>) -> EffectRun + Send + Sync + 'static,
{
    fn run(&self, issued: IssuedCmd<C>) -> EffectRun {
        (self)(issued)
    }
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegisterError {
    DuplicateActorId(ActorId),
}

struct RouteEntry {
    expected_msg_type: TypeId,
    inbox_tx: mpsc::Sender<Envelope>,
}

pub struct Runtime {
    mailbox_capacity: usize,
    router_tx: mpsc::Sender<Envelope>,
    router_rx: mpsc::Receiver<Envelope>,
    routes: HashMap<ActorId, RouteEntry>,
    actor_states: HashMap<ActorId, Box<dyn Any + Send + Sync>>,
    actor_tasks: JoinSet<()>,
    backend: Arc<dyn SystemBackend>,
    dead_letters: Arc<Mutex<Vec<DeadLetter>>>,
}

impl Runtime {
    pub fn new(mailbox_capacity: usize, backend: Arc<dyn SystemBackend>) -> Self {
        let mailbox_capacity = mailbox_capacity.max(1);
        let (router_tx, router_rx) = mpsc::channel(mailbox_capacity);
        Self {
            mailbox_capacity,
            router_tx,
            router_rx,
            routes: HashMap::new(),
            actor_states: HashMap::new(),
            actor_tasks: JoinSet::new(),
            backend,
            dead_letters: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn register_actor<A, D>(
        &mut self,
        id: ActorId,
        actor: A,
        effects: D,
    ) -> Result<Addr<A::Msg>, RegisterError>
    where
        A: Actor,
        D: EffectDriver<A::Cmd>,
    {
        if self.routes.contains_key(&id) {
            return Err(RegisterError::DuplicateActorId(id));
        }

        let (inbox_tx, inbox_rx) = mpsc::channel(self.mailbox_capacity);
        let actor_state = Arc::new(tokio::sync::Mutex::new(actor));

        self.actor_states.insert(id, Box::new(Arc::clone(&actor_state)));
        self.routes.insert(
            id,
            RouteEntry {
                expected_msg_type: TypeId::of::<A::Msg>(),
                inbox_tx,
            },
        );

        let router_tx = self.router_tx.clone();
        let backend = Arc::clone(&self.backend);
        let dead_letters = Arc::clone(&self.dead_letters);
        self.actor_tasks.spawn(run_actor_task(
            id,
            actor_state,
            effects,
            inbox_rx,
            router_tx,
            backend,
            dead_letters,
        ));

        Ok(Addr {
            id,
            tx: self.router_tx.clone(),
            marker: PhantomData,
        })
    }

    pub async fn send_envelope(&self, envelope: Envelope) -> Result<(), SendError> {
        self.router_tx
            .send(envelope)
            .await
            .map_err(|_| SendError::MailboxClosed)
    }

    pub fn actor<A>(&self, id: ActorId) -> Option<Arc<tokio::sync::Mutex<A>>>
    where
        A: Actor + 'static,
    {
        self.actor_states
            .get(&id)?
            .downcast_ref::<Arc<tokio::sync::Mutex<A>>>()
            .cloned()
    }

    pub fn dead_letters(&self) -> Vec<DeadLetter> {
        self.dead_letters
            .lock()
            .expect("dead letter collection should not be poisoned")
            .clone()
    }

    pub async fn run_for(&mut self, duration: Duration) {
        let deadline = Instant::now() + duration;
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
                maybe_env = self.router_rx.recv() => {
                    let Some(env) = maybe_env else {
                        break;
                    };
                    self.route_envelope(env).await;
                }
                maybe_done = self.actor_tasks.join_next(), if !self.actor_tasks.is_empty() => {
                    if let Some(Err(err)) = maybe_done {
                        push_dead_letter(
                            &self.dead_letters,
                            EnvelopeHeader {
                                to: ActorId(0),
                                msg_type: TypeId::of::<()>(),
                                meta: Meta::default(),
                            },
                            DeadLetterReason::EffectJoinFailed(err.to_string()),
                        );
                    }
                }
            }
        }
    }

    async fn route_envelope(&mut self, env: Envelope) {
        let header = env.header();
        let Some(route) = self.routes.get(&header.to) else {
            push_dead_letter(&self.dead_letters, header, DeadLetterReason::UnknownTarget);
            return;
        };

        if header.msg_type != route.expected_msg_type {
            push_dead_letter(&self.dead_letters, header, DeadLetterReason::TypeMismatch);
            return;
        }

        if route.inbox_tx.send(env).await.is_err() {
            push_dead_letter(&self.dead_letters, header, DeadLetterReason::MailboxClosed);
        }
    }
}

fn push_dead_letter(
    dead_letters: &Arc<Mutex<Vec<DeadLetter>>>,
    header: EnvelopeHeader,
    reason: DeadLetterReason,
) {
    dead_letters
        .lock()
        .expect("dead letter collection should not be poisoned")
        .push(DeadLetter { header, reason });
}

async fn run_actor_task<A, D>(
    actor_id: ActorId,
    actor: Arc<tokio::sync::Mutex<A>>,
    effects: D,
    mut inbox_rx: mpsc::Receiver<Envelope>,
    router_tx: mpsc::Sender<Envelope>,
    backend: Arc<dyn SystemBackend>,
    dead_letters: Arc<Mutex<Vec<DeadLetter>>>,
) where
    A: Actor,
    D: EffectDriver<A::Cmd>,
{
    let mut effect_tasks = JoinSet::<Result<Vec<Envelope>, EffectError>>::new();

    loop {
        tokio::select! {
            maybe_env = inbox_rx.recv() => {
                let Some(env) = maybe_env else {
                    break;
                };
                let header = env.header();

                let msg = match env.body.downcast::<A::Msg>() {
                    Ok(msg) => *msg,
                    Err(_) => {
                        push_dead_letter(&dead_letters, header, DeadLetterReason::DowncastFailed);
                        continue;
                    }
                };

                let cmds = {
                    let mut guard = actor.lock().await;
                    guard.update(msg)
                };

                for cmd in cmds {
                    let run = effects.run(IssuedCmd {
                        origin: actor_id,
                        meta: header.meta.clone(),
                        cmd,
                    });
                    let backend = Arc::clone(&backend);
                    effect_tasks.spawn(async move { materialize_effect_run(run, backend).await });
                }
            }
            maybe_joined = effect_tasks.join_next(), if !effect_tasks.is_empty() => {
                handle_effect_completion(maybe_joined, &router_tx, &dead_letters).await;
            }
        }
    }

    effect_tasks.abort_all();
}

async fn handle_effect_completion(
    maybe_joined: Option<Result<Result<Vec<Envelope>, EffectError>, tokio::task::JoinError>>,
    router_tx: &mpsc::Sender<Envelope>,
    dead_letters: &Arc<Mutex<Vec<DeadLetter>>>,
) {
    let Some(joined) = maybe_joined else {
        return;
    };

    match joined {
        Ok(Ok(envelopes)) => {
            for env in envelopes {
                let header = env.header();
                if router_tx.send(env).await.is_err() {
                    push_dead_letter(dead_letters, header, DeadLetterReason::MailboxClosed);
                }
            }
        }
        Ok(Err(err)) => {
            push_dead_letter(
                dead_letters,
                EnvelopeHeader {
                    to: err.origin,
                    msg_type: TypeId::of::<()>(),
                    meta: err.meta,
                },
                DeadLetterReason::EffectHandlerFailed(err.message),
            );
        }
        Err(err) => {
            push_dead_letter(
                dead_letters,
                EnvelopeHeader {
                    to: ActorId(0),
                    msg_type: TypeId::of::<()>(),
                    meta: Meta::default(),
                },
                DeadLetterReason::EffectJoinFailed(err.to_string()),
            );
        }
    }
}
