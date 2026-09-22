# Framework Follow-Ups

Exploratory suggestions, September 22, 2026. Observable semantic changes need
an ADR before implementation. Shutdown escalation is complete under
[ADR-0010](../adr/0010-shutdown-escalation.md).

- **Request lifecycles:** Opt-in timeouts and harmless late replies are complete
  under [ADR-0011](../adr/0011-request-timeouts.md). Explicit per-request
  cancellation and provider abandonment remain deferred; provider work continues
  after timeout.
- **Ordering and freshness:** Fix overlapping polls in `examples/time` with
  one request in flight or Model-owned generations. Test reordered, delayed,
  and stale outcomes; explore broader schedule variation afterward.
- **Operation identity:** Consider keyed operations where application identity
  is useful. Keep idempotency, invocation correlation, freshness, and sequencing
  distinct; require keys only when they enable a defined guarantee.
- **Bounded work:** Design admission and outstanding-work limits, including
  end-to-end pressure through Sources. Measure sustained-load memory and latency;
  leave rejection, coalescing, and shedding choices explicit.
- **Downstream Layers:** Implement a reusable reconnect/backoff composition
  outside the crate, unchanged across live and controlled execution, to discover
  the necessary public extension points.
- **Application evidence:** Exercise reconnects, competing operations,
  cancellation, and shutdown in one realistic application. Use it to assess
  Component boundaries and reduce repeated ceremony.
- **Optional integrations:** Feature-gate or split HTTP/JSON/TLS and platform
  integrations so consumers can use the runtime without unrelated dependencies.
