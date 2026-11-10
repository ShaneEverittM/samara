# ADR-0024: Component Shutdown

- Status: Accepted for implementation
- Date: November 2026
- Deciders: Samara maintainers
- Scope: `Component::shutdown`; what Drain does once the Program is quiet;
  delivery to stopped Components; abandoned requests; the shutdown report

## Context

A Component had no moment to finish. Drain let in-flight work complete and
Cancel aborted it, but nothing let a Component answer the callers whose
reply tokens it was holding or write a last line. Worse, a held reply token
kept Drain waiting forever, since an unanswered request counted as pending
work and only the escalation deadline could break the wait.

Delivering `Cancelled(Shutdown)` on effect outcomes was considered and
rejected: it would run `update`, whose Commands either run (so it was a
drain) or are discarded (so the transition was unobservable), and "the
program is stopping" is a fact about a Component, not about each effect it
has in flight. A `Stopping` Message was rejected too: every Component would
grow a variant, anything holding an address could forge it, and the runtime
would still need a hook to construct it.

## Decision

```rust,ignore
fn shutdown(
    config: &Self::Config,
    capabilities: &Capabilities<Self::Requires>,
    model: Self::Model,
) -> Command<NoMessage> {
    Command::none()
}
```

The Model is taken by value: this is the end, and held reply tokens can be
moved out and answered. The return type is finishing work (ADR-0023), so a
stopping Component may reply, send, and issue outcome-discarding effects and
cannot start anything that would come back. Termination is guaranteed by
the type. The function is pure.

1. **Modes.** Only Drain calls it. Cancel still means stop without reacting.
2. **Timing.** Stopping begins when the Program can make no further progress
   on its own: no queued messages, no effects in flight, no timers.
   Outstanding requests do not hold it back, because a held request is
   exactly the pending work that only a shutdown can resolve.
3. **Order.** Reverse registration, one Component at a time, so dependents
   stop while what they depend on is still alive.
4. **Late work.** A finishing send or reply to a still-live Component is
   delivered and that Component's Commands run normally. The runtime returns
   to the quiet condition of item 2 before stopping the next Component.
5. **After a Component has stopped.** Messages and outcomes addressed to it
   are dropped and traced. During Drain, a request whose reply token no
   longer exists anywhere (dropped by a shutdown, dropped earlier by a
   provider, or carried by a dropped message) completes for its requester
   with `RequestOutcome::Failed(RequestError::ReplyAbandoned)`. A live host
   waiter receives a `RuntimeError`.
6. **Bounding.** Finishing effects are awaited like drained work. The
   ADR-0010 escalation deadline is the only bound.
7. **Faults.** A faulted runtime calls no `shutdown`.
8. **Escalation.** If Drain escalates to Cancel, Components not yet stopped
   are cancelled without the call and `ShutdownReport::not_shut_down` names
   them.

## Consequences

- **Drain now terminates when a provider holds or has dropped a reply
  token.** This amends the ADR-0004, ADR-0006, ADR-0010, and ADR-0011
  statements that an unanswered request may keep Drain pending forever: the provider's shutdown
  either answers it or abandons it, and the requester hears which. Outside
  Drain a dropped token still leaves its request pending, as before.
- `RequestError::ReplyAbandoned` is produced for the first time, and
  `RequestOutcomeKind::Abandoned` records it in the trace.
- `TraceEvent::ComponentShutdown` and `TraceEvent::MessageDropped` are new
  controlled trace records. `ShutdownReport` gains `not_shut_down` and is no
  longer `Copy`.
- `ControlledRuntime::drain` begins a controlled Drain; stopping proceeds
  during `run_until_idle` whenever the quiet condition holds, so a test
  completes finishing effects and continues.
- No change to transition purity, per-Component serialization, or controlled
  determinism. Stop order is deterministic.

## Evidence

`tests/component_shutdown.rs`: reverse order with a finishing send landing
on a still-live peer; a held reply token answered from `shutdown`; a
dropped token abandoned and reported to its requester; a message to a
stopped Component dropped and traced; finishing effects awaited; Cancel
calling nothing; the report naming Components not stopped; and the same
held-token Drain completing live.

## Rollback

Revert the implementing commit. Drain returns to waiting on unanswered
requests.
