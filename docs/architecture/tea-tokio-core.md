# TEA + Tokio Core Architecture (v0)

## Status
- Phase: Phase 6 live-runtime implementation complete; audit ready.
- Date: July 23, 2026.
- Library scope: `samara` is library-first.

## Goals
- Combine Tokio async execution with strict The Elm Architecture (TEA) boundaries.
- Keep state evolution deterministic and inspectable.
- Keep side effects explicit, typed, and isolated from pure update logic.
- Support controlled execution, including faster-than-real-time progression for
  simulation and embedded-software testing scenarios.

## Non-Goals (v0)
- Durable event persistence and replay.
- A program-wide order for independent live events.
- Mandating a concrete mailbox, task, or event-loop topology.

## Core Contracts

### Model Contract
- `Model` represents all mutable domain state.
- No other Component or task may mutate `Model` directly.
- `Model` changes only through the runtime applying `update` output.

### Component Implementation and Configuration Contract
- A Component implementation is the Rust type and `impl Component` that define
  one kind of Component behavior.
- A configured value of that implementation contains immutable logical
  configuration and wiring, such as Ports and SourceDescriptors.
- Component methods receive that value immutably. Behaviorally relevant mutable
  state belongs in `Model`; interior mutability must not create hidden state,
  effects, or transition inputs.
- Registering the configured implementation creates a logical Component with
  stable identity and a runtime-owned Model.

### Message Contract
- `Component::Message` is the only input that may trigger a Component
  transition.
- Domain events, user intents, timer callbacks, EffectOutcomes, SourceEvents,
  RequestOutcomes, and behaviorally relevant failures enter application logic
  as Component Messages.
- A Component Message and a Protocol Message are distinct roles even when both
  are represented by enums. Normative prose qualifies the role when ambiguity
  matters.

### Command Contract
- A `Command` is an inert value describing finite work requested by a
  transition; it never performs that work itself.
- Commands may carry an EffectDescriptor and one-shot message mapper, schedule
  a Message, communicate with another Component, issue a correlated Request, or
  emit a Reply.
- Command intent must remain explicit, inspectable, and testable in isolation.
- Command and EffectDescriptor are not synonyms: a Command is the broader
  finite-work envelope.

### Update Contract
- Conceptual semantic shape:

```text
update(&self, Model, Message) -> (Model, Commands<Message>)
```

- The Phase 3 Rust signature is
  `update(&self, &mut Model, Message) -> Command<Message>`. It uses exclusively
  owned in-place Model mutation as an observationally equivalent spelling of
  the conceptual transition and one composable Command value for finite work.
- Requirements:
  - Pure: no I/O, sleep, locks, random, wall-clock, global mutable reads/writes.
  - Deterministic: for equivalent immutable Component configuration, the same
    `model` + `message` must produce equivalent state and Command intent.
  - Total for supported messages: no silent drops.

### EffectDescriptor and EffectDriver Contract
- An `EffectDescriptor` is inert typed data describing one finite world-facing
  interaction. It need not be cloneable or comparable.
- An EffectDescriptor cannot hide execution in an opaque async closure; the
  terminal world interaction remains separately identifiable to the runtime.
- A Command combines an EffectDescriptor with a pure, one-shot message mapper
  from `EffectOutcome` to the owning Component's Message.
- In live execution, an `EffectDriver<D>` is the terminal Adapter that realizes
  terminal descriptor type `D` against the surrounding world under runtime
  supervision. A non-terminal descriptor first passes through one or more
  Layers.
- Controlled execution supplies deterministic behavior for the same descriptor
  without invoking a live Driver or silently falling back to the live world.
- Each EffectOutcome actually accepted by a running scope invokes its one-shot
  message mapper exactly once and returns the resulting Message through
  runtime-managed delivery. A normally returning EffectDriver produces exactly
  one `Succeeded` or `Failed` outcome. ADR-0004 classifies whole-scope
  Cancel or runtime-fault cleanup as an abort instead: the future is cancelled,
  no EffectOutcome is manufactured, and no mapper is invoked. Drain never
  cancels an eligible finite effect merely to finish sooner.
- The exact Rust shape used to bind profile-specific Drivers and controlled
  behavior remains an API decision.

### SourceDescriptor, Subscription, and SourceDriver Contract
- A `SourceDescriptor` is inert, typed, comparable configuration describing
  ongoing event production. Equality has reconciliation semantics.
- A `Subscription` combines a stable Component-local identity, a
  SourceDescriptor, and a pure reusable message mapper from `SourceEvent` to the
  owning Component's Message. Declaring one starts no work.
- In live execution, a `SourceDriver<D>` is the terminal Adapter that realizes
  terminal descriptor type `D` against the surrounding world under runtime
  supervision. A non-terminal descriptor first passes through one or more
  Layers. Controlled execution supplies deterministic behavior for the same
  terminal descriptor without a hidden live dependency.
- A `Source` is the runtime-owned ongoing realization of a SourceDescriptor. It
  may emit zero or more SourceEvents until it ends, fails, or is canceled.
- Reconciliation starts a Source for a newly desired Subscription, retains it
  while identity and descriptor are unchanged, atomically replaces it when the
  same identity has changed configuration, and cancels it when no longer
  desired.
- A retained Source atomically adopts the latest mapper returned by the
  post-transition Subscription projection. Messages already created remain
  unchanged; later events use the new mapper.
- Each Source realization has a private runtime generation. Descriptor
  replacement is a hard cutover: old-generation events or mapped Messages that
  have not begun a Component transition are discarded and traced. An
  already-running transition completes, and no old event is mapped through the
  replacement generation.
- During reconciliation, a composed SourceDescriptor automatically lowers to a
  runtime-owned `SourcePlan` containing its terminal descriptor, ordered
  profile-independent Layers, and message mapper. Applications bind or control
  only the terminal descriptor.
- Source cancellation does not imply a synthetic SourceEvent unless the
  applicable contract explicitly promises one.
- Under ADR-0004, successful sink calls from one live Source preserve
  acceptance order. The first accepted `end`, accepted `fail`, or active
  SourceDriver return wins exactly one terminal event. Cancellation,
  replacement, shutdown, or runtime fault winning first suppresses terminal
  mapping; later sink calls return `DriverStopped`.
- Normal Source termination does not automatically restart a still-desired
  Subscription. The Source remains inactive until application desire changes
  in a way an explicit future restart policy recognizes.

### Adapter, Layer, and Driver Contract
- `Adapter` is the conceptual umbrella for code that connects declared
  boundaries; no universal `Adapter` trait is required.
- A `Layer` is a compositional Adapter. It remains inside the declarative
  boundary, behaves the same in live and controlled execution, and may own
  deterministic runtime-scoped state, but it performs no ambient I/O.
- A `Driver` is a terminal Adapter. It crosses from a descriptor stack into the
  selected live surrounding world and remains owned by the runtime scope.
- EffectDriver and SourceDriver are the currently named Driver roles. The
  taxonomy does not choose a universal Layer trait or exact profile-binding API.
- Runtime-level mechanisms remain separate from protocol and application
  policy. See `docs/architecture/effects-layering.md` for the underlying
  mechanism/policy analysis.

### Runtime Contract
- Runtime responsibilities:
  - Own message ingestion and routing.
  - Serialize transitions for each Component without requiring global serialization.
  - Execute each Component's `update` transition.
  - Interpret Commands, compose descriptors through zero or more Layers, and
    route only terminal descriptors to live Drivers or controlled behavior.
  - Apply message mappers to EffectOutcomes, SourceEvents, and RequestOutcomes.
  - Reconcile desired Subscriptions with runtime-owned Sources.
  - Compile composed SourceDescriptors into SourcePlans and enforce private
    Source-generation cutovers.
  - Record topology-neutral structural trace entries with logical time and
    causation in controlled execution.
  - Supervise task lifecycle, cancellation, and shutdown.
  - Deliver resulting Messages to their target Components.
- In the Phase 6 live profile, own one admission boundary, close it on
  shutdown or runtime fault, avoid intentional capacity drops after successful
  acceptance while the scope is healthy, and ensure fault or shutdown joins or
  aborts every owned task.
- Only runtime-managed machinery may deliver messages or execute commands;
  application Components express coordination through messages.
- Runtime driving APIs should support condition-based execution (`run_until(...)` / `run_until_predicate(...)`) and quiescence execution (`run_until_idle()`), so tests/simulations do not depend on hard-coded wall-clock sleeps.

### Component Interaction Contract (`send` / `notify` / `request`)
- Every cross-Component interaction is an explicit Command; constructing one does
  not invoke another Component during the current transition.
- `Command::send` is the lower-level one-way form for deliberate coupling to a
  target `ComponentRef<C>` and its complete `C::Message` vocabulary.
- Provider-neutral interaction uses a named `Port<P>`:
  - A value implementing `Notification<P>` is issued through `Command::notify` for
    one-way delivery.
  - A value implementing `Request<P>` declares one associated `Reply` type and
    is issued through `Command::request` for a correlated terminal outcome.
- `Command::request` includes a pure, one-shot message mapper from
  `RequestOutcome<Reply>` to the requester's ordinary Component Message. This
  request continuation may capture application-owned domain correlation.
- The runtime owns transport correlation and creates an opaque, typed
  `ReplyTo<Reply>` when it interprets a request. Components do not allocate,
  compare, or retain transport correlation identifiers.
- A provider receives one `RequestInvocation<P, R>` containing the typed Request
  and its inert `ReplyTo<R::Reply>`, then emits `Command::reply`. Consuming the
  authority expresses at-most-once reply capability without exposing a channel,
  future, or runtime handle inside `update`.
- The requester never awaits inside `update`; the mapped `RequestOutcome` returns
  through normal runtime-managed message delivery.
- Phase 5 implements the successful path only: typed Port delivery, one opaque
  transport-correlation token, at-most-once Reply, mapping to
  `RequestOutcome::Replied`, causal tracing, and one outstanding-Request
  obligation. An unanswered Request remains pending until controlled
  cancellation cleans up runtime ownership.
- Request failure, abandonment, timeout, cancellation, late-Reply, and
  delegation semantics remain deferred; Phase 5 does not manufacture those
  outcome variants.

### Component Decoupling Contract (`Port` / protocol binding)
- Reusable Component collaboration should prefer protocol-level Ports over
  concrete Component message coupling.
- A `Protocol` owns a provider-neutral `Protocol::Message` vocabulary independent
  of any provider Component's private `Component::Message` type. A concrete
  protocol enum should make that role clear, for example
  `HealthProtocolMessage`.
- A `Port<P>` is an inert, named logical dependency. It contains no provider
  reference, channel, runtime handle, or lookup capability.
- Program assembly binds each exact named Port to a provider Component whose
  Message implements `From<Protocol::Message>`. This standard conversion is
  pure and reusable rather than supplied repeatedly as a binding closure.
  Multiple named Ports of the same Protocol may be bound independently.
- `ProgramBuilder::build()` is fallible. It rejects duplicate Component
  identities, duplicate `(Protocol type, PortId)` declarations, Ports not bound
  exactly once, and providers not registered in that same builder.
- Port cycles are legal and are not detected. Assembly does not introspect
  arbitrary Component fields or behavior-dependent `Command::send` edges and
  therefore does not claim a closed static dependency graph. A send to an
  absent Component is diagnosed when interpreted.
- Swapping a real, mock, or controlled provider does not require consumer
  transition changes.
- Notification values and Request values use the symmetric `Command::notify` and
  `Command::request` entry points; only Requests add an associated Reply and
  continuation.
- Runtime-owned routing and reply resolution must preserve the delivery,
  causality, and failure semantics promised by the interaction contract without
  exposing runtime topology.

### Live Admission and Internal Pressure Contract

- Under ADR-0004, successful `ComponentHandle::send` means accepted
  for runtime-managed delivery, not that the target transition completed.
- Shutdown or runtime fault closes external admission. A racing send is either
  accepted under the chosen closure policy or rejected explicitly.
- The v0 live runtime uses unbounded internal delivery while the scope is
  healthy and running. Accepted work is not intentionally dropped because an
  internal queue filled.
- This supplies no stable capacity, fairness, throughput, latency, or
  backpressure guarantee. Sustained overload may grow memory without bound;
  memory exhaustion is unsupported.
- An upstream bounded Tokio `mpsc` channel retains its own pressure only until
  Samara receives an item. Samara adds no second bounded-pressure contract.

### Time and Controlled-Execution Contract
- The runtime must provide a clock/scheduling abstraction boundary that can support:
  - Real-time execution.
  - Logical-time execution controlled by tests or simulation harnesses.
  - Faster-than-real-time progression when controlled conditions allow.
- Timer semantics must be representable as explicit commands so scheduling can be mediated by the runtime boundary.
- Public runtime APIs must avoid forcing hard wall-clock coupling that would prevent time acceleration.
- Deterministic controlled runs must be possible with controlled clock progression.
- The v0 controlled scheduler orders runnable work by `(logical deadline,
  deterministic insertion ticket)`. Commands from one transition follow
  declaration traversal order; controlled inputs follow harness order; initial
  Component work is canonicalized by `ComponentId`, never registration order;
  and causally emitted work follows its cause.
- This equal-time rule is controlled-runtime reproducibility machinery, not a
  live or domain ordering guarantee. Cross-profile ordering requirements must
  be expressed as explicit causality.

### Controlled Trace and Work-Accounting Contract

- Controlled execution always records an in-memory structural trace that tests
  read after driving; Phase 5 requires no scheduler callback.
- Each record has a `TraceId`, `LogicalTime`, and `TraceEvent`. Initialization
  and controlled harness inputs are roots with no parent; every other record
  has exactly one immediate causal parent.
- The trace explains Component transitions; Command kinds, concrete descriptor
  types, and targets; Subscriptions and Source lifecycle including stale drops;
  terminal outcome/event shapes; logical time; and causation.
- The generic trace need not capture domain payloads. Direct typed-intent tests
  compare descriptor and Message payload values. Typed trace projection,
  streaming observers, durable storage, and replay are deferred.
- `pending_now` counts accepted Component Messages and due timers ready to run.
  `pending_later` counts pending effects, future timers, active Sources (one
  each), and outstanding Requests. Tasks, queues, locks, interpreter steps, and
  trace records are not separate obligations.
- A successful `run_until_idle()` normally returns with `pending_now == 0`;
  `pending_later` may remain nonzero. Controlled cancellation reduces both to
  zero.
- If interpretation reaches a terminal descriptor without controlled
  behavior, the drive operation fails there without invoking a live Driver or
  message mapper and records Component, work-occurrence, and descriptor-type
  context. The run is faulted: state and trace remain inspectable and
  cancellation remains available, but driving cannot resume.

### Initial Live Shutdown Contract

- Drain atomically closes external ingress, disables Source start and
  restart, stops active Sources, and recursively processes Component Messages
  and Source deliveries accepted before the cutoff plus finite work and timers
  causally emitted while draining.
- Future timers, finite effects, and Requests remain eligible. A hung Driver,
  unanswered Request, distant or recurring timer, or self-sustaining
  application may keep Drain pending forever.
- Cancel closes ingress, stops application driving, cancels queued and
  deferred semantic obligations and runtime-owned Driver tasks, then joins or
  aborts all owned tasks. It does not invoke application mappers solely because
  the scope ended.
- Successful shutdown reports
  `remaining == pending_now == pending_later == 0`. Completed and cancelled
  diagnostics count semantic obligations rather than tasks or queues, but
  their exact values remain non-normative in v0.

### Initial Live Fault Contract

- Duplicate or ambiguous live bindings fail `LiveRuntimeBuilder::build()` when
  assembly can know them.
- An unavailable terminal binding discovered only while interpreting work, an
  exhausted one-shot `mpsc` binding, a Driver panic, or an equivalent live
  mechanism violation faults the running scope.
- A live fault closes ingress, stops application driving, cancels and joins or
  aborts all runtime-owned tasks, suppresses application mappers for aborted
  work, and surfaces `RuntimeError` through subsequent ingress and the owning
  `RuntimeTask::shutdown` boundary.
- Fault cleanup does not invent typed application Error payloads. Exact fault
  taxonomy, isolation, restart, and recovery remain provisional.

### First-Party Phase 6 Bridge Contract

- First-party TCP owns one connection per Source realization, emits
  `bytes::Bytes`, maps connect/read errors to typed Source failure, maps peer
  EOF to normal ending, closes on cancellation, and contains no retry,
  reconnect, framing, or domain policy.
- One `tokio::sync::mpsc::Receiver<T>` is consumed by the first activation of
  its exact `StreamDescriptor<T>` binding. Channel closure ends normally.
  Another concurrent claimant or any later activation after end or cancellation
  faults the runtime; it does not fabricate another `Ended`.
- The `mpsc` rule is necessarily diagnostic because
  `StreamDescriptor<T>::Error` is `Infallible` and no new receiver exists to
  realize the repeated desire.
- Exact first-party public module/type names remain implementation-review
  details where the accepted API has not already frozen them.

### First-Party Standard Output Effects

- `PrintStdout` and `PrintStderr` are finite terminal EffectDescriptors, not
  ambient capabilities available to `update`.
- Live assembly opts into their Tokio Drivers through `bind_stdio`; controlled
  execution intercepts the same descriptors through the normal typed effect
  boundary.
- They are best-effort text effects with `Infallible` application Error data.
  The Driver attempts the complete write and flush but deliberately discards
  host I/O errors; cancellation may leave partial external output.
- The Drivers contain no formatting, logging, retry, or application routing
  policy and create no ordering guarantee between otherwise independent
  effects.
- A future fallible exact-byte write boundary is a separate, lower-level API.

### Decoder EOF Contract

- ADR-0004 adds one required pure Decoder finalization operation.
- Only normal underlying `Ended` invokes finalization, exactly once and after
  all earlier accepted chunks from that Source.
- Final frames are emitted in order before outer `Ended`. A finalization Error
  emits one `FramedError::Decode` failure and no `Ended`.
- Underlying Source failure, an earlier decoder failure, cancellation,
  replacement, shutdown, and runtime fault do not invoke finalization.
- First-party TCP chunks use `bytes::Bytes`; decoder state may use
  `bytes::BytesMut`.

## Error Channel Design
- Error names typed explanatory data, such as `TcpError` or `RequestError`.
  Failure names the semantic occurrence carrying Error data. Normal Source
  ending and cancellation are distinct terminal conditions, not failures.
- Expected behaviorally relevant failures for which a boundary contract defines
  typed Error data must be representable as Component Messages,
  EffectOutcomes, SourceEvents, or RequestOutcomes where they occur.
- Driver panics and live mechanism faults cannot truthfully construct arbitrary
  typed application Error data. ADR-0004 surfaces them as
  `RuntimeError` after initiating structured scope cleanup.
- The exact boundary variant taxonomy remains a focused API decision.
- No panic-based control flow for expected errors.

## Message, Effect, and Source Lifecycles
1. A Component Message enters runtime-managed delivery for a target Component.
2. The runtime selects it according to per-Component serialization, causality,
   and any explicitly promised sequencing contract.
3. The runtime calls the target Component's pure transition using its immutable
   configuration, current Model, and Message, then commits the resulting Model.
4. The runtime interprets emitted Commands and reconciles desired Subscriptions.
5. An EffectDescriptor passes through zero or more Layers. Its terminal
   descriptor reaches a live EffectDriver or controlled behavior. Every
   accepted EffectOutcome passes through the composed path exactly once and its
   one-shot mapper produces one Component Message. Whole-scope live abort may
   instead cancel the Driver without manufacturing an outcome.
6. A desired SourceDescriptor passes through zero or more Layers. Its terminal
   descriptor, ordered Layers, and mapper compile into a SourcePlan. The
   terminal descriptor reaches a live SourceDriver or controlled behavior,
   producing a runtime-owned Source whose SourceEvents return through the same
   Layers and repeatedly pass through the current reusable message mapper.
7. Resulting Messages return through runtime-managed delivery, and execution
   continues until the applicable lifecycle, fault, or shutdown policy
   triggers.

## Topology Status
- Runtime topology is not prescribed for v0.
- A single loop, one loop per Component, a hybrid scheduler, or another design may conform.
- Each Component's transitions are serialized; causal and explicitly promised sequencing guarantees are preserved.
- Independent live events have no implicit program-wide order.
- Controlled execution selects a deterministic schedule and produces a reproducible program-wide trace.
- ADR-0003 defines the v0 controlled equal-time schedule, Source cutover,
  structural trace, validation, and semantic work-accounting rules.
- ADR-0004 defines no extra global live order. Phase 6 evidence accepts
  either order for independent completions and compares causal partial order,
  not controlled trace-vector order.
- Internal topology may change without an ADR when these observable semantics remain unchanged.
- See `docs/architecture/topology-options.md`,
  `docs/adr/0002-runtime-topology-and-ordering.md`, and
  `docs/adr/0003-controlled-execution-semantics.md`. Accepted live behavior is
  in
  `docs/adr/0004-initial-live-runtime-semantics.md`.

## Related Design Sketches
- Thin-slice API comparison and PoC shape: `docs/architecture/thin-slice-value.md`.
- Effect mechanism/policy split: `docs/architecture/effects-layering.md`.
