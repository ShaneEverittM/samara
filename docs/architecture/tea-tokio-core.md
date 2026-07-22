# TEA + Tokio Core Architecture (v0)

## Status
- Phase: Documentation-only.
- Date: February 28, 2026.
- Library scope: `samara` is library-first.

## Goals
- Combine Tokio async execution with strict The Elm Architecture (TEA) boundaries.
- Keep state evolution deterministic and inspectable.
- Keep side effects explicit, typed, and isolated from pure update logic.
- Support simulation-oriented execution, including faster-than-real-time progression for embedded-software testing scenarios.

## Non-Goals (v0)
- Durable event persistence and replay.
- A program-wide order for independent live events.
- Mandating a concrete mailbox, task, or event-loop topology.

## Core Contracts

### Model Contract
- `Model` represents all mutable domain state.
- No other Component or task may mutate `Model` directly.
- `Model` changes only through the runtime applying `update` output.

### Msg Contract
- `Msg` is the only trigger for state transitions.
- Domain events, user intents, timer callbacks, and runtime failures are represented as `Msg`.

### Cmd Contract
- `Cmd` is a typed enum representing effect intents.
- `Cmd` describes work to perform; it does not perform work itself.
- `Cmd` variants must be serializable in thought and testable in isolation (even if not serialized in v0).

### Update Contract
- Canonical shape:

```rust
fn update(model: Model, msg: Msg) -> (Model, Vec<Cmd>);
```

- Requirements:
  - Pure: no I/O, sleep, locks, random, wall-clock, global mutable reads/writes.
  - Deterministic: same `model` + `msg` must produce same output.
  - Total for supported messages: no silent drops.

### Effect Handler Contract
- Canonical shape:

```rust
async fn handle(cmd: Cmd) -> Result<Vec<Msg>, RuntimeError>;
```

- Effect handlers run on Tokio.
- Handler output is transformed into `Msg` values and returned through runtime-managed message delivery.
- Handler failures must become structured runtime error messages.
- Effect layering rule:
  - Runtime-level effects define mechanism.
  - Adapter/protocol and app layers define policy.
  - See `docs/architecture/effects-layering.md`.

### Runtime Contract
- Runtime responsibilities:
  - Own message ingestion and routing.
  - Serialize transitions for each Component without requiring global serialization.
  - Execute each Component's `update` transition.
  - Dispatch resulting `Cmd` values to effect handlers.
  - Supervise task lifecycle, cancellation, and shutdown.
  - Deliver handler output messages to their target Components.
- Only runtime-managed machinery may deliver messages or execute commands;
  application Components express coordination through messages.
- Runtime driving APIs should support condition-based execution (`run_until(...)` / `run_until_predicate(...)`) and quiescence execution (`run_until_idle()`), so tests/simulations do not depend on hard-coded wall-clock sleeps.

### Component Interaction Contract (`send` / `notify` / `request`)
- Every cross-Component interaction is an explicit `Cmd`; constructing one does
  not invoke another Component during the current transition.
- `Cmd::send` is the lower-level one-way form for deliberate coupling to a
  target `ComponentRef<C>` and its complete `C::Msg` vocabulary.
- Provider-neutral interaction uses a named `Port<P>`:
  - A value implementing `Notification<P>` is issued through `Cmd::notify` for
    one-way delivery.
  - A value implementing `Request<P>` declares one associated `Reply` type and
    is issued through `Cmd::request` for a correlated terminal outcome.
- `Cmd::request` includes a pure result mapper from `RequestOutcome<Reply>` to
  the requester's ordinary `Msg`. The mapper is the continuation and may capture
  application-owned domain correlation.
- The runtime owns transport correlation and creates an opaque, typed
  `ReplyTo<Reply>` when it interprets a request. Components do not allocate,
  compare, or retain transport correlation identifiers.
- A provider receives the request with its inert `ReplyTo`, then emits
  `Cmd::reply`. Consuming the token expresses one-shot reply authority without
  exposing a channel, future, or runtime handle inside `update`.
- The requester never awaits inside `update`; the mapped `RequestOutcome` returns
  through normal runtime-managed message delivery.
- Request delivery, abandonment, timeout, and cancellation must be explicit
  terminal outcomes where applicable. This contract does not select a default
  deadline or cancellation policy.

### Component Decoupling Contract (`Port` / protocol binding)
- Reusable Component collaboration should prefer protocol-level Ports over
  concrete Component message coupling.
- A `Protocol` owns a provider-neutral inbound vocabulary independent of any
  provider Component's private `Msg` type.
- A `Port<P>` is an inert, named logical dependency. It contains no provider
  reference, channel, runtime handle, or lookup capability.
- Program assembly binds each exact named Port to a provider Component using a
  pure mapping from the Protocol's inbound vocabulary to the provider's `Msg`.
  Multiple named Ports of the same Protocol may be bound independently.
- Swapping a real, mock, or controlled provider does not require consumer
  transition changes.
- Notification values and Request values use the symmetric `Cmd::notify` and
  `Cmd::request` entry points; only Requests add an associated Reply and result
  continuation.
- Runtime-owned routing and reply resolution must preserve the delivery,
  causality, and failure semantics promised by the interaction contract without
  exposing runtime topology.

### Time and Simulation Contract
- The runtime must provide a clock/scheduling abstraction boundary that can support:
  - Real-time execution.
  - Simulated-time execution controlled by tests/simulation harnesses.
  - Faster-than-real-time progression when simulation conditions allow.
- Timer semantics must be representable as explicit commands so scheduling can be mediated by the runtime boundary.
- Public runtime APIs must avoid forcing hard wall-clock coupling that would prevent time acceleration.
- Deterministic simulation runs must be possible with controlled clock progression.

## Error Channel Design
- Domain failures:
  - Represented by domain-level `Msg` variants (example: `Msg::DomainError(...)`).
- Runtime/executor failures:
  - Represented by runtime-level `Msg` variants (example: `Msg::RuntimeError(...)`).
- No panic-based control flow for expected errors.

## Message and Effect Lifecycle
1. `Msg` enters runtime-managed delivery for a target Component.
2. Runtime selects it according to per-Component serialization, causality, and any explicitly promised sequencing contract.
3. Runtime calls the target Component's pure `update(model, msg)`.
4. Runtime commits next `Model`.
5. Runtime dispatches each `Cmd` to Tokio effect handlers.
6. Effect handlers complete and emit `Msg` values.
7. Runtime delivers emitted `Msg` values to their target Components.
8. Execution continues until shutdown policy triggers.

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
