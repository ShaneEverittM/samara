# ADR 0010: Shutdown Escalation

- Status: Accepted
- Date: September 21, 2026
- Extends: [ADR-0004](0004-initial-live-runtime-semantics.md) and
  [ADR-0007](0007-live-host-lifecycle.md)

## Context

Drain can wait indefinitely for an effect, unanswered request, or recurring
timer. The consuming `shutdown` API prevents the host from timing out Drain,
escalating to Cancel, and awaiting cleanup through the same owner.

## Decision

Add a synchronous, non-consuming shutdown request:

```rust,ignore
impl RuntimeTask {
    pub fn request_shutdown(&mut self, mode: Shutdown);
}
```

`request_shutdown` closes external ingress before returning but does not wait
for cleanup. `run_forever` remains the cancellation-safe completion observer;
`shutdown(self, mode)` becomes a convenience that requests shutdown and awaits
the same terminal result.

- Running accepts Drain or Cancel; Draining may escalate to Cancelling.
- Repeated requests are harmless. Drain cannot reverse Cancel. Requests after
  fault or closure leave the terminal result unchanged.
- Cancelling observation preserves ownership and the requested shutdown state.
- Existing fault arbitration remains unchanged: shutdown cannot replace a
  preserved fault with success. Observation reports faults after cleanup.
- Successful completion guarantees zero remaining runtime-owned work.
  Requesting shutdown alone provides no completion guarantee.

The host chooses its grace period using ordinary Tokio composition:

```rust,ignore
task.request_shutdown(Shutdown::Drain);
match tokio::time::timeout(grace_period, task.run_forever()).await {
    Ok(result) => result,
    Err(_) => {
        task.request_shutdown(Shutdown::Cancel);
        task.run_forever().await
    }
}
```

## Invariants and Limits

Component purity, message delivery, and controlled execution are unchanged.
Drain retains its existing work semantics; Cancel may discard that work and
creates no synthetic application outcomes. Dropping the owner retains the
existing abort fallback. Grace expiry bounds the wait for Drain, not cleanup
duration. Request deadlines, per-operation cancellation, and signal policy are
outside this decision.

## Required Evidence

- Drain escalates to Cancel and closes all owned work for a pending effect,
  unanswered request, and recurring timer.
- Cancelling observation retains the owner and permits escalation and joining.
- Repeated requests, Cancel followed by Drain, and requests after completion
  preserve monotonic shutdown and the original terminal result.
- Ingress rejects after the request returns; fault/escalation races preserve
  existing fault arbitration; the host example compiles.

## Rollback

Revert the additive API and escalation semantics together. Hosts return to
selecting Drain or Cancel up front; existing consuming shutdown remains usable.
