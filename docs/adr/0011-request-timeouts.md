# ADR 0011: Request Timeouts

- Status: Accepted
- Date: September 22, 2026
- Extends: [ADR-0003](0003-controlled-execution-semantics.md) and
  [ADR-0006](0006-live-port-ingress.md)

## Context

An unanswered Request owns its continuation indefinitely and prevents Drain
from finishing. An application timer or cancelled host waiter cannot release
that obligation.

## Decision

Add opt-in timeout forms, retaining existing unbounded calls:

```rust,ignore
Command::request_timeout(port, request, duration) // canonical From conversion
Command::request_timeout_with(port, request, duration, mapper)
port_handle.request_timeout(request, duration).await
    // Result<RequestOutcome<R::Reply>, RuntimeError>
```

Component timeouts start when the runtime interprets the Request Command.
Host timeouts start at admission on the future's first poll, including time
queued for runtime interpretation. Controlled execution uses logical time;
live execution uses Tokio time. Unrepresentable deadlines fail explicitly.

- A Reply settles successfully only when interpreted strictly before the
  deadline. At or after it, timeout wins, including a zero duration. This is
  a reply-acceptance deadline, not a real-time response-latency guarantee.
- Expiry removes the outstanding Request and its timeout bookkeeping, then
  emits exactly one `TimedOut` outcome through the existing continuation or
  host waiter. Reply success instead removes the unused timeout.
- A late Reply from that expired Request is discarded without another outcome
  or runtime fault. Foreign or otherwise invalid reply authority still faults.
  Expiry must not leave a runtime-owned tombstone for an absent provider.
- Timeout does not withdraw an admitted provider Message or cancel provider
  effects, timers, or state changes. Those retain their ordinary ownership.
- Drain processes deadlines and resulting Messages. Expired Requests no longer
  hold Drain open, but independent provider work may do so.
- Scope Cancel/fault keeps its existing abort semantics: it clears requests and
  deadlines without manufacturing Component outcomes. Host scope failures
  remain `RuntimeError`. Dropping a host waiter does not cancel its Request.

Controlled traces distinguish replies, timeouts, and discarded late replies.
Timeouts have the issuing Request as their causal parent and count as Request
mechanism, not an extra timer obligation. Explicit cancellation, provider
abandonment detection, default deadlines, and retry remain deferred.

## Evidence and Risks

Tests cover both profiles: reply before expiry, no reply, late reply, equality
at the deadline, zero duration, overflow, concurrent correlation, removed
deadline work, and scope cancellation. Live tests also cover host admission
time, dropped waiters, Drain, and fault preservation. Controlled tests verify
trace causality, reproducibility, and single-obligation accounting.

The provider may still perform an external action after its caller times out;
applications must express any stop/retry/idempotency policy separately.
Component purity and per-Component serialization remain unchanged.

## Rollback

Remove the timeout forms and restore successful-reply-only interpretation
together. Existing unbounded request calls remain supported; applications using
the new forms must migrate explicitly before rollback.
