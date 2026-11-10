# ADR-0025: A Bare Effect's Outcome Is Its Driver's Result

- Status: Accepted. Amends ADR-0020.
- Date: November 2026
- Deciders: Samara maintainers
- Scope: The shape of `EffectOutcome`; removal of `EffectFailure` and
  `CancelReason`

## Context

ADR-0020 gave every effect outcome a `Cancelled` case so different effects
could share one failure handler:

```rust,ignore
enum EffectOutcome<T, E> { Succeeded(T), Failed(EffectFailure<E>) }
enum EffectFailure<E>    { Error(E), Cancelled(CancelReason) }
```

and noted that it "adds no new source of live per-effect cancellation". None
has appeared. Drivers return `Result`, so a live effect ends in success or
its Driver's error. Whole-scope Cancel drops mappers without calling them.
ADR-0024 made shutdown a method on the Component, so shutdown is not an
outcome in any mode. The only way a Component ever received `Cancelled` was
a controlled test supplying it, which tested a case the runtime cannot
produce.

The cost was real: every continuation for an effect that cannot fail still
had to handle a failure arm. `ReadSystemTime` has `Error = Infallible` and
its outcome was still refutable.

Cancellation is also not a property of an effect. It is done to one, by a
deadline, by newer work, by a scope ending. The effect's own type cannot
know what surrounds it.

## Decision

```rust,ignore
pub enum EffectOutcome<T, E> {
    Succeeded(T),
    Failed(E),
}
```

A bare effect's outcome is success or its Driver's error. `EffectFailure`
and `CancelReason` are removed. `EffectOutcomeKind` loses `Cancelled`.

For an effect whose error is uninhabited the failure case does not exist,
so the continuation can take the success directly:

```rust,ignore
Effect::perform(clock, ReadSystemTime).producing(|EffectOutcome::Succeeded(now)| Message::Now(now))
```

When a mechanism that cancels an effect is designed (a deadline, a
supersession key), it adds its reason to the outcome type at the place the
Component asks for it, the way the HTTP response pipeline already widens the
error as steps are added. The Component that asks for a deadline is then the
one, and the only one, that must handle it.

## Alternatives considered

- **Let a descriptor declare its whole failure type** and set it to
  `Infallible`. That makes "nothing may cancel me" a promise the descriptor
  cannot keep on behalf of whatever wraps it.
- **A `producing_ok` finisher** for infallible effects. Sugar over the same
  promise.
- **Keep `Cancelled` for controlled tests.** A test that supplies an outcome
  the runtime cannot produce verifies nothing.

## Consequences

- Continuations for infallible effects are irrefutable. Fallible effects
  lose one nesting level: `Failed(error)` rather than
  `Failed(EffectFailure::Error(error))`.
- ADR-0020's shared failure handler across output types is still available,
  since `Failed(E)` carries no output type either.
- The controlled trace no longer records `EffectOutcomeKind::Cancelled`.
- No change to purity, serialization, ordering, or determinism.

## Evidence

The clock rustdoc and `tests/effect_outcomes.rs` show the irrefutable
continuation for `ReadSystemTime`; every existing effect suite passes with
the flattened outcome; pulse's journal handles one failure arm, not two.

## Rollback

Revert the implementing commit.
