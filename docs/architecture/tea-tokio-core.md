# TEA + Tokio Core Architecture (v0)

## Status
- Phase: Phase 4 declarative-work kernel accepted; Phase 5 controlled execution ready.
- Date: July 22, 2026.
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
- Each issued descriptor reaches exactly one `EffectOutcome`. The runtime invokes
  its one-shot message mapper exactly once and returns the resulting Message
  through runtime-managed delivery.
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
  while identity and descriptor are unchanged, replaces or reconfigures it when
  the same identity has changed configuration, and cancels it when no longer
  desired.
- Whether a newly declared message mapper replaces the prior mapper while a
  Source is retained remains an explicit API decision.
- Source cancellation does not imply a synthetic SourceEvent unless the
  applicable contract explicitly promises one.

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
  - Supervise task lifecycle, cancellation, and shutdown.
  - Deliver resulting Messages to their target Components.
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
- Request delivery, abandonment, timeout, and cancellation must be explicit
  terminal outcomes where applicable. This contract does not select a default
  deadline or cancellation policy.

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
- Swapping a real, mock, or controlled provider does not require consumer
  transition changes.
- Notification values and Request values use the symmetric `Command::notify` and
  `Command::request` entry points; only Requests add an associated Reply and
  continuation.
- Runtime-owned routing and reply resolution must preserve the delivery,
  causality, and failure semantics promised by the interaction contract without
  exposing runtime topology.

### Time and Controlled-Execution Contract
- The runtime must provide a clock/scheduling abstraction boundary that can support:
  - Real-time execution.
  - Logical-time execution controlled by tests or simulation harnesses.
  - Faster-than-real-time progression when controlled conditions allow.
- Timer semantics must be representable as explicit commands so scheduling can be mediated by the runtime boundary.
- Public runtime APIs must avoid forcing hard wall-clock coupling that would prevent time acceleration.
- Deterministic controlled runs must be possible with controlled clock progression.

## Error Channel Design
- Error names typed explanatory data, such as `TcpError` or `RequestError`.
  Failure names the semantic occurrence carrying Error data. Normal Source
  ending and cancellation are distinct terminal conditions, not failures.
- Behaviorally relevant domain and runtime failures must be representable as
  typed Component Messages, EffectOutcomes, SourceEvents, or RequestOutcomes at
  the boundary where they occur.
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
   descriptor reaches a live EffectDriver or controlled behavior, produces
   exactly one EffectOutcome through the composed path, and its one-shot
   message mapper produces one Component Message.
6. A desired SourceDescriptor passes through zero or more Layers. Its terminal
   descriptor reaches a live SourceDriver or controlled behavior, producing a
   runtime-owned Source whose SourceEvents return through the same Layers and
   repeatedly pass through its reusable message mapper.
7. Resulting Messages return through runtime-managed delivery, and execution
   continues until the applicable lifecycle or shutdown policy triggers.

## Topology Status
- Runtime topology is not prescribed for v0.
- A single loop, one loop per Component, a hybrid scheduler, or another design may conform.
- Each Component's transitions are serialized; causal and explicitly promised sequencing guarantees are preserved.
- Independent live events have no implicit program-wide order.
- Controlled execution selects a deterministic schedule and produces a reproducible program-wide trace.
- Internal topology may change without an ADR when these observable semantics remain unchanged.
- See `docs/architecture/topology-options.md` and `docs/adr/0002-runtime-topology-and-ordering.md`.

## Related Design Sketches
- Thin-slice API comparison and PoC shape: `docs/architecture/thin-slice-value.md`.
- Effect mechanism/policy split: `docs/architecture/effects-layering.md`.
