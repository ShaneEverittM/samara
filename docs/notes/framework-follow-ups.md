# Framework Follow-Ups

Exploratory suggestions, September 22, 2026. Observable semantic changes need
an ADR before implementation. Shutdown escalation is complete under
[ADR-0010](../adr/0010-shutdown-escalation.md).

Teaching principles for the future guide live in [User Guide Notes](user-guide-notes.md).

- **Request lifecycles:** Opt-in timeouts and harmless late replies are complete
  under [ADR-0011](../adr/0011-request-timeouts.md). Explicit per-request
  cancellation and provider abandonment remain deferred; provider work continues
  after timeout.
- **Ordering and freshness:** `examples/time` now uses an Idle/Polling Model
  enum, skips busy ticks, and tests delayed outcomes and both tick/completion
  orders. The
  [before/after API review](first-real-application.md#fifth-follow-through-polling-ordering-and-freshness)
  recommends explicit polling guidance and common terminal cleanup. Broader
  schedule variation and reusable concurrency APIs remain exploratory.
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

## URL Checker API Exercise — September 23, 2026

Open items from writing the application by hand; these are exploratory proposals,
not accepted API decisions or a review of the unfinished application.

- [ ] **Example-led rustdoc:** Make Source construction discoverable without
  navigating several types. Show a small, complete path from declaring a Program
  capability, storing it on a Component, and returning a Subscription to binding
  its live implementation. Include the controlled equivalent and handling for
  `SourceEvent::Item`, `Ended`, and `Failed`. Link the recipe from the relevant
  constructors and traits, and compile-check the examples as doctests.
- [ ] **Explicit Subscription restart revision:** Explore a Model-owned revision
  that the Component can increment to request a fresh Source realization with
  unchanged configuration. Consider keeping the stable Subscription identity
  separate from this revision; `with_revision(...)` is only a candidate spelling.
  Specify replacement, cleanup, and stale-event behavior using the existing
  private-generation cutover contract. Keep retry/backoff policy in Components
  or Layers and preserve deterministic controlled execution. Document binding
  limitations: TCP can reconnect, while a consumed one-shot `mpsc` receiver
  cannot be recreated by incrementing a revision. Test retention, restart,
  stale-event rejection, and unsupported reactivation before implementation.
- [ ] **Component-requested completion:** Explore a typed way for a Component to
  report that it is finished, motivated by the checker's stdin EOF. Distinguish
  local Component completion from application termination; consider program
  assembly designating which completion signals should end the application,
  with minimal wiring for a single-Component CLI. Define whether pending checks,
  timers, and output finish or are cancelled, how completion reaches the host,
  and how runtime-owned cleanup and faults are reported. Make the same intent
  observable in controlled execution. Specify and test EOF with outstanding
  work and completion in a multi-Component application before implementation.

The lifecycle proposals require ADRs and matching acceptance evidence before
changing observable semantics. Existing contracts are described in
[subscription reconciliation and Source bridges](../api-guidance.md),
[host lifecycle](../adr/0007-live-host-lifecycle.md), and
[shutdown escalation](../adr/0010-shutdown-escalation.md).
