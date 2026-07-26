# Samara Agent Rules

## Scope
This repository is in a documentation-first, acceptance-contract-gated implementation phase for a Tokio + TEA (The Elm Architecture) runtime library.

## Non-Negotiable Architecture Invariants
- `Model` is the single source of truth.
- `Message` is the only path for state transitions.
- The semantic transition `Model + Message -> Model + Commands` is pure,
  deterministic, and side effect free. The Phase 3 Rust spelling is
  `update(&self, &mut Model, Message) -> Command<Message>`; in-place mutation is
  exclusively owned and one composable Command represents zero or more intents.
- `Command` is a typed enum describing finite work. World-facing work remains explicit as typed `EffectDescriptor` values; opaque async closures are not allowed in v0.
- Live side effects run only through Drivers on runtime-owned Tokio tasks.
- Async code must not mutate state directly; it can only emit `Message` values back through runtime-managed message delivery.
- Runtime APIs must preserve controlled execution with logical time, including faster-than-real-time test runs.
- Runtime-owned mechanisms and Drivers must remain mechanism-only; protocol/app policy belongs in Layers or application logic.

## Effect and Source Vocabulary
- `EffectDescriptor` is an inert, typed description of one finite world-facing interaction.
- `EffectOutcome` is the single terminal success, failure, or cancellation produced for one issued `EffectDescriptor`.
- `Error` names typed explanatory data; `Failure` names the semantic occurrence carrying it. Normal ending and cancellation are terminal conditions, not failures.
- `SourceDescriptor` is an inert, typed, reconciliation-comparable description of ongoing event production.
- `Subscription` is a Component's model-derived desire to maintain a `SourceDescriptor` under stable Component-local identity, together with its event-to-Message mapper.
- `Source` is the runtime-scoped realization of a `SourceDescriptor` maintained for an active `Subscription`.
- `Layer` is the canonical term for a compositional transformation that remains inside the declared effect/source model and can run unchanged in live and controlled execution.
- `Driver` is the canonical term for a terminal live-world implementation of an effect or source boundary.
- `Adapter` is a conceptual umbrella used in user guidance; concrete architecture roles should be named `Layer` or `Driver`.
- Controlled execution supplies deterministic effect outcomes and source events without silently invoking live Drivers.

## Runtime Semantics Rule
- ADR `docs/adr/0002-runtime-topology-and-ordering.md` does not mandate a mailbox, task, or event-loop topology.
- Each Component's transitions must be serialized and non-overlapping.
- Causal relationships and explicitly promised FIFO/sequencing guarantees must be preserved.
- Independent live events have no implicit program-wide order.
- Controlled execution must produce a deterministic program-wide trace for identical controlled inputs and runtime semantics.
- Public APIs, application logic, and conformance tests must not depend on incidental scheduler topology or global serialization.
- Changes to observable ordering, causality, isolation, or controlled-determinism semantics require an ADR and matching evidence.

## Delivery Workflow (Maximum Rigor)
1. Specify or update architecture contracts in docs before code.
2. For architecture-impacting changes, create or update an ADR first.
3. Write tests first for:
   - State transitions.
   - Command emission.
   - Runtime behavior for affected flows.
4. Implement in small vertical slices with explicit acceptance criteria.
5. Update invariants, failure modes, and rollback notes in the same PR.

## Core Do's
- Keep `update` free from I/O, blocking, locks, and Tokio runtime handles.
- Model domain and runtime failures as explicit `Message` variants.
- Keep command semantics explicit and typed.
- Preserve per-Component serialization, causal ordering, and controlled-execution determinism.
- Add traceability between requirement, design artifact, tests, and implementation.
- Isolate time behind runtime-owned scheduling abstractions so test harnesses can control clock progression.
- Split large effect behavior into composable Layers and protocol modules plus narrow terminal Drivers rather than one monolithic handler.

## Core Don'ts
- No hidden side effects in reducers or update paths.
- No untyped background task spawning outside runtime/Driver boundaries.
- No direct state mutation from async tasks.
- No architecture change merged without matching test and invariant updates.
- No "temporary" bypass of message flow.
- No hard-coding wall-clock assumptions into reusable runtime APIs.
- No embedding protocol-specific retry/reconnect/framing policy into runtime mechanism or terminal Drivers.

## Progress Patterns
- Pattern A: `Message` and state transition table -> tests -> implementation.
- Pattern B: `Command` and `EffectDescriptor` semantics spec -> Driver and controlled-behavior tests -> runtime integration.
- Pattern C: ADR proposal → tradeoff matrix → explicit decision → implementation.

## Required PR Contents
- Invariant impact summary.
- Failure mode analysis.
- Rollback/recovery notes.
- Test coverage summary tied to changed behavior.
- Links to updated architecture docs and ADRs (when applicable).

## Out of Scope for v0
- Durable persistence and replay are explicitly excluded from v0 implementation scope.
