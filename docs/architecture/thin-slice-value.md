# Thin Slice of Value: API Sketch and Runtime Options

## Goal
Define a minimal proof of concept that exercises:
- One actor.
- Two effects.
- Strict TEA boundaries with Option A runtime semantics.

This is an API-shape comparison document, not implementation code.

## Thin Slice Scenario
Use a `Counter` actor with:
- `Model`: current count and last error.
- `Msg`: user increments, timer ticks, and effect completion/failure.
- `Cmd`: persist count and schedule next tick.

### Shared Domain Sketch
```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CounterModel {
    pub count: u64,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CounterMsg {
    IncrementRequested,
    Tick,
    Persisted,
    PersistFailed(String),
    RuntimeFailed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CounterCmd {
    PersistCount(u64),
    ScheduleTick(std::time::Duration),
}
```

## 1) Methods vs Free Functions

### Option M1: Stateful method (`&mut self`)
```rust
pub struct CounterActor {
    pub model: CounterModel,
}

impl CounterActor {
    pub fn on_msg(&mut self, msg: CounterMsg) -> Vec<CounterCmd> {
        match msg {
            CounterMsg::IncrementRequested => {
                self.model.count += 1;
                vec![
                    CounterCmd::PersistCount(self.model.count),
                    CounterCmd::ScheduleTick(std::time::Duration::from_secs(1)),
                ]
            }
            CounterMsg::PersistFailed(err) => {
                self.model.last_error = Some(err);
                vec![]
            }
            _ => vec![],
        }
    }
}
```

Notes:
- Ergonomic for OO-style usage.
- Weakens TEA purity signaling because mutation is implicit and easier to abuse.

### Option M2: Pure method (`self` by value)
```rust
impl CounterModel {
    pub fn update(self, msg: CounterMsg) -> (Self, Vec<CounterCmd>) {
        match msg {
            CounterMsg::IncrementRequested => {
                let next = Self { count: self.count + 1, ..self };
                let cmds = vec![
                    CounterCmd::PersistCount(next.count),
                    CounterCmd::ScheduleTick(std::time::Duration::from_secs(1)),
                ];
                (next, cmds)
            }
            CounterMsg::PersistFailed(err) => {
                (Self { last_error: Some(err), ..self }, vec![])
            }
            _ => (self, vec![]),
        }
    }
}
```

Notes:
- Preserves purity while keeping method call ergonomics.
- Slightly less explicit than a top-level update contract.

### Option F1: Free function (TEA canonical)
```rust
pub fn update(model: CounterModel, msg: CounterMsg) -> (CounterModel, Vec<CounterCmd>) {
    match msg {
        CounterMsg::IncrementRequested => {
            let next = CounterModel { count: model.count + 1, ..model };
            let cmds = vec![
                CounterCmd::PersistCount(next.count),
                CounterCmd::ScheduleTick(std::time::Duration::from_secs(1)),
            ];
            (next, cmds)
        }
        CounterMsg::PersistFailed(err) => {
            (CounterModel { last_error: Some(err), ..model }, vec![])
        }
        _ => (model, vec![]),
    }
}
```

Notes:
- Strongest TEA signal and easiest to unit-test as pure input/output.
- Most explicit architecture boundary for contributors and agent automation.

### Recommendation
- Use **F1 (free function)** as the default public contract.
- Allow M2 as an internal style only when it compiles to the same pure semantics.
- Avoid M1 for v0 runtime-facing code.

## 2) Should Non-Effect Handlers Be `async`?

### Option H1: Non-effect handlers are synchronous (recommended)
```rust
pub fn update(model: CounterModel, msg: CounterMsg) -> (CounterModel, Vec<CounterCmd>) {
    // Pure, deterministic, no await points.
    # (model, vec![])
}

pub async fn handle_effect(cmd: CounterCmd) -> Result<Vec<CounterMsg>, RuntimeError> {
    // All async I/O and waiting live here.
    # Ok(vec![])
}
```

Why:
- Prevents accidental I/O, sleep, or lock waits in update paths.
- Keeps deterministic tests straightforward.
- Preserves clear architecture boundaries.

### Option H2: Make update async (not recommended)
```rust
pub async fn update(model: CounterModel, msg: CounterMsg) -> (CounterModel, Vec<CounterCmd>) {
    // await points here blur effect boundaries.
    # (model, vec![])
}
```

Why not:
- Weakens TEA constraints.
- Makes determinism and timing behavior harder to reason about.
- Encourages hidden side effects.

### Recommendation
- Keep all non-effect handlers synchronous in v0.
- If compute is expensive, emit a command and execute in a Tokio task (or `spawn_blocking`) instead of making `update` async.

## 3) Option A Runtime Implementation Shapes

All options below preserve:
- Single mailbox loop.
- Global total order for message processing in the runtime loop.

### Runtime R1: Simple spawn-per-command
```rust
pub async fn run(mut model: CounterModel, mut rx: tokio::sync::mpsc::Receiver<CounterMsg>) {
    let tx = /* cloneable sender */;

    while let Some(msg) = rx.recv().await {
        let (next, cmds) = update(model, msg);
        model = next;

        for cmd in cmds {
            let tx2 = tx.clone();
            tokio::spawn(async move {
                match handle_effect(cmd).await {
                    Ok(msgs) => for m in msgs { let _ = tx2.send(m).await; },
                    Err(err) => { let _ = tx2.send(CounterMsg::RuntimeFailed(err.to_string())).await; }
                }
            });
        }
    }
}
```

Tradeoffs:
- Simple to understand and ship quickly.
- Weak task supervision and shutdown control if many tasks are detached.

### Runtime R2: JoinSet-supervised effects (recommended thin slice)
```rust
pub async fn run(mut model: CounterModel, mut rx: tokio::sync::mpsc::Receiver<CounterMsg>) {
    let tx = /* cloneable sender */;
    let mut effects = tokio::task::JoinSet::<Vec<CounterMsg>>::new();

    loop {
        tokio::select! {
            maybe_msg = rx.recv() => {
                let Some(msg) = maybe_msg else { break; };
                let (next, cmds) = update(model, msg);
                model = next;

                for cmd in cmds {
                    effects.spawn(async move {
                        match handle_effect(cmd).await {
                            Ok(msgs) => msgs,
                            Err(err) => vec![CounterMsg::RuntimeFailed(err.to_string())],
                        }
                    });
                }
            }
            joined = effects.join_next(), if !effects.is_empty() => {
                if let Some(Ok(msgs)) = joined {
                    for msg in msgs {
                        let _ = tx.send(msg).await;
                    }
                }
            }
        }
    }
}
```

Tradeoffs:
- Better lifecycle management and clearer shutdown/cancellation behavior.
- Slightly more runtime complexity than R1.

### Runtime R3: Bounded command worker pool
```rust
// Loop emits Cmd into bounded queue; fixed worker tasks handle effects and send Msgs back.
// Good for backpressure control when command rates are high.
```

Tradeoffs:
- Stronger throughput/backpressure control.
- More moving pieces for a first proof of concept.

## 4) Open Set Messages and Targets in a Library Runtime

### Problem
A library cannot assume one fixed global `AppMsg` enum known ahead of time. It must support:
- New actor types added by downstream users.
- New message types per actor without changing core runtime code.

### Option O1: Closed-world global message enum
```rust
pub enum AppMsg {
    Counter(counter::Msg),
    Timer(timer::Msg),
    Runtime(RuntimeMsg),
}
```

Tradeoffs:
- Best compile-time exhaustiveness.
- Not open-set friendly for a reusable library; every new actor/message requires editing the root enum.

### Option O2: Typed addresses + erased runtime envelope (recommended)
```rust
use std::any::{Any, TypeId};
use std::marker::PhantomData;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ActorId(pub u64);

pub struct Envelope {
    pub to: ActorId,
    pub msg_type: TypeId,
    pub body: Box<dyn Any + Send>,
    pub meta: Meta,
}

pub struct Addr<M> {
    id: ActorId,
    tx: tokio::sync::mpsc::Sender<Envelope>,
    _marker: PhantomData<fn(M)>,
}

impl<M: Send + 'static> Addr<M> {
    pub async fn send(&self, msg: M) -> Result<(), SendError> {
        let env = Envelope {
            to: self.id,
            msg_type: TypeId::of::<M>(),
            body: Box::new(msg),
            meta: Meta::new(),
        };
        self.tx.send(env).await.map_err(|_| SendError::MailboxClosed)
    }
}
```

Runtime registration/dispatch sketch:
```rust
pub trait Actor: Send + 'static {
    type Msg: Send + 'static;
    fn on_msg(&mut self, msg: Self::Msg) -> Vec<Cmd>;
}

pub trait ErasedActor: Send {
    fn actor_id(&self) -> ActorId;
    fn msg_type(&self) -> TypeId;
    fn dispatch(&mut self, env: Envelope) -> Result<Vec<Cmd>, DispatchError>;
}

pub struct Runtime {
    registry: std::collections::HashMap<ActorId, Box<dyn ErasedActor>>,
    dead_letters: Vec<DeadLetter>,
}
```

Dispatch behavior:
1. Dequeue `Envelope`.
2. Lookup `to` in `registry`.
3. If not found, emit `RuntimeMsg::UnknownTarget` and record dead letter.
4. If found but type mismatch, emit `RuntimeMsg::TypeMismatch` and record dead letter.
5. If matched, dispatch to actor and run returned `Cmd` effects.

Dead-letter and runtime message sketch:
```rust
pub enum RuntimeMsg {
    UnknownTarget { to: ActorId },
    TypeMismatch { to: ActorId },
    MailboxClosed,
    EffectFailed(String),
}

pub struct DeadLetter {
    pub reason: RuntimeMsg,
    pub envelope_meta: Meta,
}
```

Why O2 fits this repository:
- Keeps public sending API type-safe (`Addr<M>::send(M)`).
- Keeps runtime extensible with open-set actor/message registration.
- Preserves Option A single-mailbox semantics while allowing plugin-like growth.

Risk and mitigation:
- Risk: internal type erasure can hide mistakes.
- Mitigation: strict runtime diagnostics (`RuntimeMsg`) and dead-letter capture by default.

## Thin Slice Recommendation
- API shape:
  - `update` as a free function.
  - `handle_effect` as async.
  - Domain model/message/command types explicit and strongly typed.
  - Use `Addr<M>` for typed sends and `Envelope`+registry internally for open-set routing.
- Runtime shape:
  - Start with **R2 JoinSet-supervised Option A loop**.
- Effects to include in first PoC:
  - `PersistCount(u64)` (simulated async persistence).
  - `ScheduleTick(Duration)` (timer-based message emission).

## Success Criteria for the PoC
- Public API shows a clear TEA contract (`Model`, `Msg`, `Cmd`, `update`).
- At least two commands execute via async handlers and re-enter as messages.
- Runtime surfaces effect failures as `RuntimeFailed` messages.
- Unknown-target and type-mismatch dispatches produce structured runtime messages and dead letters.
- Deterministic update tests pass for the actor state transitions.
