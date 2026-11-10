# ADR-0020: Effect Failure Reasons

- Status: Accepted
- Date: November 9, 2026
- Deciders: Samara maintainers
- Scope: Group effect errors and explicit cancellation under one unsuccessful
  outcome; amend the effect vocabulary and the outcome spelling in ADR-0004.

## Context

A requester first needs to know whether an effect produced its result. When it
did not, error and cancellation are distinct reasons that application policy
may handle together or separately. The flat three-variant `EffectOutcome`
forces a mapper either to handle both reasons immediately or to forward an
entire outcome whose type still permits success and depends on the output type.

Journal demonstrates the desired separation: a callback routes successful
bytes to parsing, while a failure Message carries the unsuccessful reason to
an update arm that maps it into Journal's domain error.

## Decision

```rust,ignore
pub enum EffectOutcome<T, E> {
    Succeeded(T),
    Failed(EffectFailure<E>),
}

pub enum EffectFailure<E> {
    Error(E),
    Cancelled(CancelReason),
}
```

`Failed` means that the operation did not produce a successful result.
`EffectFailure<E>` has no output parameter and cannot represent success, so
effects with different output types can share a failure handler. Both types
remain ordinary owned data, deriving Clone, Debug, PartialEq, and Eq with the
usual payload bounds. `EffectFailure` is exported in the prelude.

An Error remains typed explanatory data. Explicit effect cancellation is an
unsuccessful completion, whether requested by the caller or another party.
The nested reason preserves that distinction; cancellation does not become
an I/O error or automatically imply retry, fault logging, or a runtime fault.
Normal Source ending remains a normal terminal condition. Source and Request
outcome shapes are unchanged.

Drivers still return `Result<Output, Error>`: `Ok` becomes `Succeeded`, and
`Err(error)` becomes `Failed(EffectFailure::Error(error))`. Controlled
execution accepts either failure reason. HTTP response transformations wrap
their own errors and preserve cancellation without running success steps.

The diagnostic `EffectOutcomeKind` continues to distinguish `Succeeded`,
`Failed` (the Error reason), and `Cancelled`. Existing traces retain their
categories. Whole-scope Cancel still stops application driving and drops
mappers without manufacturing an effect outcome. This representation change
adds no new source of live per-effect cancellation.

## Invariants and Failure Modes

- No change to transition purity, effect ownership, per-Component
  serialization, causal ordering, controlled determinism, or exactly-once
  terminal delivery.
- Failure mapping stays pure; domain policy belongs in the application.
- Exhaustive matches must migrate both reasons. Classifying every outer
  `Failed` as an error would erase cancellation diagnostics; trace and HTTP
  tests must cover the nested reasons independently.
- `EffectOutcome<T, Infallible>` still permits cancellation, so only its
  `EffectFailure::Error(never)` arm is uninhabited.

## Acceptance Evidence

- `tests/effect_outcomes.rs`: success routes retain their output types, while
  errors and cancellation reach a shared failure-only Message and are mapped
  to domain data in update; controlled traces preserve both reasons.
- `tests/phase4_declarative_work.rs` and `tests/from_continuations.rs`: pure
  mapper inspection and canonical Message conversion retain payloads.
- `tests/phase5_controlled_execution.rs` and
  `tests/phase6_live_runtime.rs`: controlled cancellation, live/controlled
  error parity, and whole-scope cancellation without synthesized outcomes.
- `tests/http_response_pipeline.rs` and `tests/http_redirects.rs`: errors and
  cancellation propagate through response processing and redirect handling.
- `examples/pulse/src/journal/tests.rs`: read errors and explicit cancellation
  reach Journal's centralized domain mapping and reply with distinct public
  errors; append outcomes are discarded without producing another transition.

## Migration and Rollback

Replace `EffectOutcome::Failed(error)` with
`EffectOutcome::Failed(EffectFailure::Error(error))`, and replace
`EffectOutcome::Cancelled(reason)` with
`EffectOutcome::Failed(EffectFailure::Cancelled(reason))`. Applications may
then combine both branches as `EffectOutcome::Failed(reason)`.

Revert the enum change and migrate matches back to the flat shape to roll
back. No persisted state or wire format changes; diagnostic trace categories
are preserved.
