# First Real Application Learnings

- Status: Trimmed to unlanded findings
- Date: July 25, 2026; trimmed November 8, 2026
- Evidence: [`examples/time`](../../examples/time/src/main.rs)

This note recorded what writing `examples/time`, the first application built
outside Samara's acceptance examples, taught about the API. Its host lifecycle,
closed capability, HTTP, stdio, stdin, and polling conclusions have landed in
ADR-0005 through ADR-0009, in [API Guidance](../api-guidance.md) under "How Do
I Poll Without Overlapping Effects?", and in the example's own tests in
[`main.rs`](../../examples/time/src/main.rs) and
[`polling_tests.rs`](../../examples/time/src/polling_tests.rs). It now holds
only findings not yet recorded anywhere else; live follow-ups moved to
[Framework Follow-Ups](framework-follow-ups.md).

## Persistent State in Manual Effect Drivers

Some Drivers need operational state shared across effect invocations: a pooled
HTTP client, authenticated session, device handle, or long-lived connection.
The registered `EffectDriver` object already persists for the live runtime's
lifetime and can hold such state. However, `execute(&self)` returns an owned
`'static` future, so custom Drivers commonly need cloneable handles or
`Arc`-backed interior state to move access into that future.

The first-party HTTP Driver provides concrete evidence for this model: one
registered Driver instance retains a cloneable reqwest client, and sequential
effects reuse its pool. No additional runtime-owned state-store abstraction was
needed for that case.

We should determine whether this existing persistent Driver instance is a
sufficient escape hatch once documented and exemplified, or whether manual
Drivers need a more ergonomic runtime-owned store/context. Any design must make
concurrency semantics explicit: silently serializing all invocations through
one mutable store would be a consequential policy, not a free convenience.

## Interval SourceDescriptor

`HttpTimeServer` expresses periodic polling with finite timer Commands,
scheduling `Command::after` in `init` and scheduling another timer whenever the
tick Message is handled. The ongoing desire to receive periodic ticks could
instead be a natural Subscription:

```rust,ignore
Subscription::source_with(
    &self.interval,
    Interval::every(Duration::from_secs(1)),
    |_| Message::RefreshDue,
)
```

A first-party interval SourceDescriptor would give live execution Tokio time
and controlled execution logical time through the same Component program. It
would also stop naturally when the Subscription is withdrawn or the runtime
begins Drain.

The semantics need care. A fixed-rate interval, a fixed delay after completion,
and "never overlap requests" are different application intents. We must define
the first tick, missed-tick behavior, coalescing, and controlled-time behavior
rather than copying Tokio defaults accidentally. The time-subscription shape
itself is tracked under "Time access beyond delayed delivery" in
[Framework Follow-Ups](framework-follow-ups.md); these semantics apply to it.

## Effect Completion Order and Terminal Output

The CLI makes one ordering boundary concrete. Two input lines reach the
Component in FIFO order and their transitions do not overlap, but a print
Effect from the first transition and a request/reply chain from the second are
independent asynchronous work. The later reply's presentation may therefore
reach the stdio Driver before the earlier print. That is consistent with the
current no-implicit-effect-completion-order contract. A CLI requiring ordered
transcript output would need to model that sequencing explicitly, for example
by waiting for each mapped print outcome before issuing the next output, rather
than depending on incidental runtime scheduling. The simple example does not
claim that stronger policy.

The ordering prerequisite proposed for first-party filesystem effects in
[Framework Follow-Ups](framework-follow-ups.md), where a sink Driver opts in to
per-Component issue order, would cover stdio as well; the discarded-outcome
print macros otherwise leave an application no outcome to wait for.

## Fifth Follow-Through: Polling Ordering and Freshness

The before/after evidence from the September 22 review is now the test list in
[`polling_tests.rs`](../../examples/time/src/polling_tests.rs) and the recipes
in [API Guidance](../api-guidance.md). The API conclusions that remain open:

**Recommendation:** keep the runtime signatures and ordering semantics. One
example does not justify a generic concurrency API with implicit operation
identity or stale-result policy.

A future polling Layer/helper would need to name cadence, overlap, missed ticks,
terminal cleanup, and effect cancellation explicitly. An interval Source alone
would not fix overlap; an idempotency key alone would not establish freshness.
Keep those as design questions until another application demonstrates repeated
policy scaffolding.
