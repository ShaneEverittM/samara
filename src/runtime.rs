use std::{
    any::type_name,
    any::{Any, TypeId},
    collections::HashMap,
    future::Future,
    marker::PhantomData,
    pin::Pin,
    sync::{Arc, Mutex, RwLock},
};

use tokio::{
    sync::{mpsc, oneshot},
    task::JoinSet,
    time::{Duration, Instant},
};

use crate::system_effects::{EffectRun, SystemBackend, materialize_effect_run};

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AskError {
    MailboxClosed,
    ResponseChannelClosed,
}

impl From<SendError> for AskError {
    fn from(value: SendError) -> Self {
        match value {
            SendError::MailboxClosed => Self::MailboxClosed,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeTellError {
    ActorTypeNotRegistered(&'static str),
    MailboxClosed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeAskError {
    ActorTypeNotRegistered(&'static str),
    MailboxClosed,
    ResponseChannelClosed,
}

impl From<AskError> for RuntimeAskError {
    fn from(value: AskError) -> Self {
        match value {
            AskError::MailboxClosed => Self::MailboxClosed,
            AskError::ResponseChannelClosed => Self::ResponseChannelClosed,
        }
    }
}

pub struct ActorRef<A: Actor> {
    id: ActorId,
    tx: mpsc::Sender<Envelope>,
    marker: PhantomData<fn(A)>,
}

impl<A: Actor> Clone for ActorRef<A> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            tx: self.tx.clone(),
            marker: PhantomData,
        }
    }
}

impl<A: Actor> ActorRef<A> {
    pub fn actor_id(&self) -> ActorId {
        self.id
    }

    pub async fn tell(&self, msg: A::Msg) -> Result<(), SendError> {
        self.tell_with_meta(msg, Meta::default()).await
    }

    pub async fn tell_with_meta(&self, msg: A::Msg, meta: Meta) -> Result<(), SendError> {
        let env = Envelope::with_meta(self.id, msg, meta);
        self.tx
            .send(env)
            .await
            .map_err(|_| SendError::MailboxClosed)
    }

    pub async fn send(&self, msg: A::Msg) -> Result<(), SendError> {
        self.tell(msg).await
    }

    pub async fn send_with_meta(&self, msg: A::Msg, meta: Meta) -> Result<(), SendError> {
        self.tell_with_meta(msg, meta).await
    }

    pub async fn ask<R, Build>(&self, build: Build) -> Result<R, AskError>
    where
        R: Send + 'static,
        Build: FnOnce(oneshot::Sender<R>) -> A::Msg,
    {
        self.ask_with_meta(build, Meta::default()).await
    }

    pub async fn ask_with_meta<R, Build>(&self, build: Build, meta: Meta) -> Result<R, AskError>
    where
        R: Send + 'static,
        Build: FnOnce(oneshot::Sender<R>) -> A::Msg,
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tell_with_meta(build(reply_tx), meta)
            .await
            .map_err(AskError::from)?;
        reply_rx.await.map_err(|_| AskError::ResponseChannelClosed)
    }

    pub fn downgrade(&self) -> WeakActorRef<A> {
        WeakActorRef {
            id: self.id,
            tx: self.tx.downgrade(),
            marker: PhantomData,
        }
    }
}

pub struct WeakActorRef<A: Actor> {
    id: ActorId,
    tx: mpsc::WeakSender<Envelope>,
    marker: PhantomData<fn(A)>,
}

impl<A: Actor> Clone for WeakActorRef<A> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            tx: self.tx.clone(),
            marker: PhantomData,
        }
    }
}

impl<A: Actor> WeakActorRef<A> {
    pub fn actor_id(&self) -> ActorId {
        self.id
    }

    pub fn upgrade(&self) -> Option<ActorRef<A>> {
        let tx = self.tx.upgrade()?;
        Some(ActorRef {
            id: self.id,
            tx,
            marker: PhantomData,
        })
    }
}

#[derive(Clone)]
pub struct RuntimeRef {
    router_tx: mpsc::Sender<Envelope>,
    actor_type_index: Arc<RwLock<HashMap<TypeId, ActorId>>>,
}

impl RuntimeRef {
    pub async fn send_envelope(&self, envelope: Envelope) -> Result<(), SendError> {
        self.router_tx
            .send(envelope)
            .await
            .map_err(|_| SendError::MailboxClosed)
    }

    pub fn actor_ref<A>(&self) -> Option<ActorRef<A>>
    where
        A: Actor + 'static,
    {
        let id = *self
            .actor_type_index
            .read()
            .expect("actor type index lock should not be poisoned")
            .get(&TypeId::of::<A>())?;
        Some(ActorRef {
            id,
            tx: self.router_tx.clone(),
            marker: PhantomData,
        })
    }

    pub fn weak_actor_ref<A>(&self) -> Option<WeakActorRef<A>>
    where
        A: Actor + 'static,
    {
        self.actor_ref::<A>().map(|strong| strong.downgrade())
    }

    pub async fn tell<A>(&self, msg: A::Msg) -> Result<(), RuntimeTellError>
    where
        A: Actor + 'static,
    {
        let actor_ref = self
            .actor_ref::<A>()
            .ok_or(RuntimeTellError::ActorTypeNotRegistered(type_name::<A>()))?;
        actor_ref
            .tell(msg)
            .await
            .map_err(|_| RuntimeTellError::MailboxClosed)
    }

    pub async fn ask<A, R, Build>(&self, build: Build) -> Result<R, RuntimeAskError>
    where
        A: Actor + 'static,
        R: Send + 'static,
        Build: FnOnce(oneshot::Sender<R>) -> A::Msg,
    {
        let actor_ref = self
            .actor_ref::<A>()
            .ok_or(RuntimeAskError::ActorTypeNotRegistered(type_name::<A>()))?;
        actor_ref.ask(build).await.map_err(RuntimeAskError::from)
    }
}

#[derive(Clone)]
pub struct UpdateContext {
    actor_id: ActorId,
    runtime: RuntimeRef,
}

impl UpdateContext {
    pub fn new(actor_id: ActorId, runtime: RuntimeRef) -> Self {
        Self { actor_id, runtime }
    }

    pub fn actor_id(&self) -> ActorId {
        self.actor_id
    }

    pub fn runtime(&self) -> &RuntimeRef {
        &self.runtime
    }
}

#[derive(Clone)]
pub struct EffectContext {
    actor_id: ActorId,
    runtime: RuntimeRef,
}

impl EffectContext {
    pub fn new(actor_id: ActorId, runtime: RuntimeRef) -> Self {
        Self { actor_id, runtime }
    }

    pub fn actor_id(&self) -> ActorId {
        self.actor_id
    }

    pub fn runtime(&self) -> &RuntimeRef {
        &self.runtime
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

    fn update(&mut self, msg: Self::Msg, ctx: &UpdateContext) -> Vec<Self::Cmd>;

    fn effect_driver(context: Self::DriverContext) -> Self::Driver
    where
        Self: Sized;
}

pub trait EffectDriver<C>: Send + Sync + 'static {
    fn run(&self, issued: IssuedCmd<C>, ctx: &EffectContext) -> EffectRun;
}

impl<C, F> EffectDriver<C> for F
where
    C: Send + 'static,
    F: Fn(IssuedCmd<C>, &EffectContext) -> EffectRun + Send + Sync + 'static,
{
    fn run(&self, issued: IssuedCmd<C>, ctx: &EffectContext) -> EffectRun {
        (self)(issued, ctx)
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
    DuplicateActorType(&'static str),
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
    actor_type_index: Arc<RwLock<HashMap<TypeId, ActorId>>>,
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
            actor_type_index: Arc::new(RwLock::new(HashMap::new())),
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
    ) -> Result<ActorRef<A>, RegisterError>
    where
        A: Actor,
        D: EffectDriver<A::Cmd>,
    {
        if self.routes.contains_key(&id) {
            return Err(RegisterError::DuplicateActorId(id));
        }
        let actor_type = TypeId::of::<A>();
        if self
            .actor_type_index
            .read()
            .expect("actor type index lock should not be poisoned")
            .contains_key(&actor_type)
        {
            return Err(RegisterError::DuplicateActorType(type_name::<A>()));
        }

        let (inbox_tx, inbox_rx) = mpsc::channel(self.mailbox_capacity);
        let actor_state = Arc::new(tokio::sync::Mutex::new(actor));

        self.actor_states
            .insert(id, Box::new(Arc::clone(&actor_state)));
        self.routes.insert(
            id,
            RouteEntry {
                expected_msg_type: TypeId::of::<A::Msg>(),
                inbox_tx,
            },
        );
        self.actor_type_index
            .write()
            .expect("actor type index lock should not be poisoned")
            .insert(actor_type, id);

        let router_tx = self.router_tx.clone();
        let runtime_ref = self.runtime_ref();
        let backend = Arc::clone(&self.backend);
        let dead_letters = Arc::clone(&self.dead_letters);
        self.actor_tasks.spawn(run_actor_task(
            id,
            actor_state,
            effects,
            inbox_rx,
            router_tx,
            runtime_ref,
            backend,
            dead_letters,
        ));

        Ok(ActorRef {
            id,
            tx: self.router_tx.clone(),
            marker: PhantomData,
        })
    }

    pub async fn send_envelope(&self, envelope: Envelope) -> Result<(), SendError> {
        self.runtime_ref().send_envelope(envelope).await
    }

    pub async fn tell<A>(&self, msg: A::Msg) -> Result<(), RuntimeTellError>
    where
        A: Actor + 'static,
    {
        self.runtime_ref().tell::<A>(msg).await
    }

    pub async fn ask<A, R, Build>(&self, build: Build) -> Result<R, RuntimeAskError>
    where
        A: Actor + 'static,
        R: Send + 'static,
        Build: FnOnce(oneshot::Sender<R>) -> A::Msg,
    {
        self.runtime_ref().ask::<A, R, Build>(build).await
    }

    pub fn runtime_ref(&self) -> RuntimeRef {
        RuntimeRef {
            router_tx: self.router_tx.clone(),
            actor_type_index: Arc::clone(&self.actor_type_index),
        }
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

    pub fn actor_ref<A>(&self) -> Option<ActorRef<A>>
    where
        A: Actor + 'static,
    {
        let id = *self
            .actor_type_index
            .read()
            .expect("actor type index lock should not be poisoned")
            .get(&TypeId::of::<A>())?;
        let route = self.routes.get(&id)?;
        if route.expected_msg_type != TypeId::of::<A::Msg>() {
            return None;
        }

        Some(ActorRef {
            id,
            tx: self.router_tx.clone(),
            marker: PhantomData,
        })
    }

    pub fn weak_actor_ref<A>(&self) -> Option<WeakActorRef<A>>
    where
        A: Actor + 'static,
    {
        let strong = self.actor_ref::<A>()?;
        Some(strong.downgrade())
    }

    pub fn dead_letters(&self) -> Vec<DeadLetter> {
        self.dead_letters
            .lock()
            .expect("dead letter collection should not be poisoned")
            .clone()
    }

    pub async fn run(&mut self) {
        loop {
            tokio::select! {
                maybe_env = self.router_rx.recv() => {
                    let Some(env) = maybe_env else {
                        break;
                    };
                    self.route_envelope(env).await;
                }
                maybe_done = self.actor_tasks.join_next(), if !self.actor_tasks.is_empty() => {
                    self.handle_actor_task_join(maybe_done);
                }
            }
        }
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
                    self.handle_actor_task_join(maybe_done);
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

    fn handle_actor_task_join(&self, maybe_done: Option<Result<(), tokio::task::JoinError>>) {
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
    runtime_ref: RuntimeRef,
    backend: Arc<dyn SystemBackend>,
    dead_letters: Arc<Mutex<Vec<DeadLetter>>>,
) where
    A: Actor,
    D: EffectDriver<A::Cmd>,
{
    let update_ctx = UpdateContext::new(actor_id, runtime_ref.clone());
    let effect_ctx = EffectContext::new(actor_id, runtime_ref);
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
                    guard.update(msg, &update_ctx)
                };

                for cmd in cmds {
                    let run = effects.run(IssuedCmd {
                        origin: actor_id,
                        meta: header.meta.clone(),
                        cmd,
                    }, &effect_ctx);
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
