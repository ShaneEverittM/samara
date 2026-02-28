# Samara Agent Rules

## Scope
This repository is in a documentation-first architecture phase for a Tokio + TEA (The Elm Architecture) runtime library.

## Non-Negotiable Architecture Invariants
- `Model` is the single source of truth.
- `Msg` is the only path for state transitions.
- `update(Model, Msg) -> (Model, Vec<Cmd>)` is pure, deterministic, and side-effect free.
- `Cmd` is a typed enum of effect intents. Opaque async closures are not allowed in v0.
- Side effects run only in effect handlers on Tokio tasks.
- Async code must not mutate state directly; it can only emit `Msg` values back to the runtime mailbox.

## Runtime Topology Rule
- v0 runtime topology is fixed by ADR `docs/adr/0001-runtime-topology.md`:
  - Option A (single mailbox loop).
  - Global total order semantics.
- Implementation must conform to Option A unless a superseding ADR is accepted.
- Proposals to move to hybrid or actor topology must include migration evidence and updated ordering guarantees.

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
- Model domain and runtime failures as explicit `Msg` variants.
- Keep command semantics explicit and typed.
- Preserve deterministic processing guarantees as documented for the selected topology.
- Add traceability between requirement, design artifact, tests, and implementation.

## Core Don'ts
- No hidden side effects in reducers or update paths.
- No untyped background task spawning outside runtime/effect-handler boundaries.
- No direct state mutation from async tasks.
- No architecture change merged without matching test and invariant updates.
- No "temporary" bypass of message flow.

## Progress Patterns
- Pattern A: `Msg` and state transition table -> tests -> implementation.
- Pattern B: `Cmd` semantics spec -> effect handler tests -> runtime integration.
- Pattern C: ADR proposal -> tradeoff matrix -> explicit decision -> implementation.

## Required PR Contents
- Invariant impact summary.
- Failure mode analysis.
- Rollback/recovery notes.
- Test coverage summary tied to changed behavior.
- Links to updated architecture docs and ADRs (when applicable).

## Out of Scope for v0
- Durable persistence and replay are explicitly excluded from v0 implementation scope.
