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
- Changing runtime topology without a superseding ADR.

## Core Contracts

### Model Contract
- `Model` represents all mutable domain state.
- No external actor or task may mutate `Model` directly.
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
- Handler output is transformed into `Msg` values and re-enqueued into the runtime mailbox.
- Handler failures must become structured runtime error messages.
- Effect layering rule:
  - Runtime-level effects define mechanism.
  - Adapter/protocol and app layers define policy.
  - See `docs/architecture/effects-layering.md`.

### Runtime Contract
- Runtime responsibilities:
  - Own mailbox ingestion.
  - Pull one `Msg` at a time according to selected ordering policy.
  - Execute `update`.
  - Dispatch resulting `Cmd` values to effect handlers.
  - Supervise task lifecycle, cancellation, and shutdown.
  - Re-enqueue handler output messages.
- Runtime must be the only component coordinating message flow and command execution.
- Runtime driving APIs should support condition-based execution (`run_until(...)` / `run_until_predicate(...)`) and quiescence execution (`run_until_idle()`), so tests/simulations do not depend on hard-coded wall-clock sleeps.

### Actor Interaction Contract (`tell` / `ask`)
- `tell` is a one-way message send (`ActorRef::tell`), with delivery acknowledgment only (`MailboxClosed` on failure).
- `ask` is request/reply over message flow (`ActorRef::ask` / `ActorRef::ask_request`), with runtime-owned reply transport.
- For request-style APIs, runtime supports typed request metadata via `Message<Actor>`:
  - Request types implement `Message<A>` with associated `Reply`.
  - Runtime APIs (`tell_request` / `ask_request`) infer reply behavior from the request type and attach runtime-owned reply tokens.
  - Internal actor `Msg` enums remain domain-focused and do not need explicit reply channel fields.
- `update` remains pure for `ask` handling:
  - `update` may emit a typed `Cmd` containing reply intent only (no reply field required).
  - `update` claims reply lifecycle from context (`claim_reply`) when a deferred reply is expected.
  - Effect handlers perform the actual reply side effect using runtime metadata helpers (`reply_from_issued` / `reply_from_meta`).
- No direct I/O or reply-channel sending in `update`.
- Type-based actor linking (`RuntimeRef::tell` / `RuntimeRef::ask`) must preserve the same mailbox and error semantics.

### Actor Decoupling Contract (`Port` / protocol binding)
- Multi-actor collaboration should prefer protocol-level ports over concrete actor type lookup.
- `Port` contracts define stable request/response types independent of provider actor implementations.
- Runtime binding maps `Port` to a provider actor at startup (`real` vs `mock`), so swapping implementations does not require consumer code changes.
- Provider actors implement protocol adapters (`PortHandler::request`) to translate a request into actor `Msg` without branching on `ask` vs `tell`.
- Runtime owns request-reply transport details for port calls.
  - `ask` attaches a waiting runtime reply slot.
  - `tell` attaches a detached runtime reply slot that discards the response.
  - `update` reads an opaque `ReplyToken` from `UpdateContext`; effect handlers resolve it through runtime APIs.
  - Actor message types stay protocol-shaped and do not carry concrete reply channels.
- Port calls (`PortRef::tell` / `PortRef::ask`) must preserve the same mailbox and failure semantics as direct actor refs.

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
1. `Msg` enters mailbox.
2. Runtime dequeues per selected ordering guarantees.
3. Runtime calls pure `update(model, msg)`.
4. Runtime commits next `Model`.
5. Runtime dispatches each `Cmd` to Tokio effect handlers.
6. Effect handlers complete and emit `Msg` values.
7. Runtime enqueues emitted `Msg` values.
8. Loop continues until shutdown policy triggers.

## Topology Status
- Topology is decided for v0: **Option A (Single App Mailbox)**.
- Message ordering guarantee for v0 is **global total order**.
- Runtime implementation must enforce a single mailbox update loop, with async handler output re-entering via `Msg`.
- Any move to hybrid or actor topology requires a superseding ADR with migration evidence.
- See `docs/architecture/topology-options.md` and `docs/adr/0001-runtime-topology.md`.

## Related Design Sketches
- Thin-slice API comparison and PoC shape: `docs/architecture/thin-slice-value.md`.
- Effect mechanism/policy split: `docs/architecture/effects-layering.md`.
