# ADR 0002: Runtime Topology and Ordering Semantics

- Status: Accepted
- Date: July 21, 2026
- Decision owners: Samara maintainers
- Supersedes: [ADR 0001](0001-runtime-topology.md)

## Context

ADR-0001 selected a single application mailbox and a program-wide total order
for v0. That was a useful simplifying assumption for an early proof of concept,
but it coupled Samara's observable semantics to one scheduler shape.

Samara's vision now defines a Component as a topology-neutral unit of state and
behavior. Component isolation, explicit causality, and reproducible controlled
execution provide the reasoning guarantees Samara needs without imposing a
global order on independent live events. Requiring global serialization would
also encourage accidental coordination between Components and unnecessarily
limit implementation choices for throughput, latency, and fault isolation.

This ADR replaces the topology and ordering decision in ADR-0001.

## Decision Drivers

- Preserve pure, deterministic Component transitions.
- Make Component state ownership and cross-Component coordination explicit.
- Preserve causal guarantees that application code can rely upon.
- Guarantee program-wide reproducibility in controlled execution.
- Permit independent Components to make progress independently in live
  execution.
- Keep public APIs and application logic independent of scheduler topology.
- Allow simple and more concurrent runtime implementations to compete on
  implementation evidence rather than on different application semantics.

## Decision

Samara does not prescribe a runtime mailbox, task, thread, or event-loop
topology. A conforming runtime may use a single dispatcher, one loop per
Component, a hybrid scheduler, or another internal design.

Every conforming runtime must preserve these observable semantics:

- Each Component has a serialized transition history. Two transitions for the
  same Component never overlap.
- Component state changes only through typed message delivery and a committed
  transition.
- Causal ordering is preserved. At minimum:
  - An effect outcome follows the command that requested the effect.
  - Message delivery follows the send that requested delivery.
  - A reply follows its request.
  - Explicitly sequenced or FIFO work preserves only the ordering promised by
    its contract.
- Independent Components and independent live events have no implicit relative
  order. Samara exposes no program-wide total-order guarantee.
- Controlled execution selects a deterministic schedule. Given the same
  program, controlled world, inputs, seeds, and runtime semantics, it produces
  the same program-wide semantic trace and final state.
- The order selected by a controlled scheduler is reproducibility machinery,
  not a domain-ordering guarantee for live execution.
- Public APIs, application logic, and conformance tests must not depend on
  incidental topology or scheduling details.

A single global loop remains a conforming implementation technique, but any
incidental serialization it creates is not part of Samara's semantic contract.
Likewise, a per-Component or hybrid runtime must not weaken the guarantees
above.

State requiring atomic invariants across multiple conceptual concerns belongs
in one Component or behind an explicit coordinating Component. Runtime
scheduling is not an implicit transaction mechanism.

## Change Policy

An implementation may change its internal topology without a superseding ADR
when the observable guarantees above remain unchanged. A change to public
ordering, causality, isolation, or controlled-determinism semantics requires an
ADR and corresponding conformance evidence.

Topology-specific tuning and diagnostics are permitted, but portable Component
logic must not require them for correctness.

## Consequences

### Positive

- Runtime topology can evolve for throughput, latency, isolation, or simplicity
  without changing the programming model.
- Component boundaries encode state ownership instead of relying on accidental
  global serialization.
- Independent Components may progress concurrently.
- Live and controlled runtimes may use different scheduling mechanisms while
  preserving the same Component semantics.
- A simple single-loop implementation remains available without becoming a
  permanent public contract.

### Negative

- Applications cannot infer atomicity or coordination across Components from
  runtime scheduling.
- Tests cannot rely on incidental ordering between independent events.
- Runtime traces and tests must express causality rather than treating one
  convenient global sequence as semantic.
- Delivery, FIFO, fairness, backpressure, and overload behavior must be explicit
  contracts wherever applications depend on them.
- Concurrent implementations require more careful lifecycle, load, and fault
  testing than a single global loop.

## Considered Alternatives

### Mandate a Single Global Mailbox

Rejected as a semantic requirement. It provides a convenient total order but
constrains throughput and can encourage Components to rely on accidental global
serialization. It remains a valid implementation strategy.

### Mandate One Loop per Component

Rejected as a semantic requirement. It naturally expresses state ownership and
independent progress, but task and mailbox allocation are implementation
choices. It remains a valid implementation strategy.

### Mandate a Hybrid Root and Child Topology

Rejected as a semantic requirement. It may be useful for some workloads, but
Samara does not currently need to expose that structure to application code.

## Required Evidence

A conforming implementation must demonstrate:

- Concurrent delivery to one Component never produces overlapping transitions.
- A multi-Component scenario preserves command-to-outcome, send-to-delivery, and
  request-to-reply causal edges without asserting an order between independent
  live events.
- Explicit FIFO or sequencing behavior is tested wherever it is promised.
- Repeated controlled runs with identical program, world, input, seed, and
  runtime semantics produce equivalent program-wide traces and final state.
- The same representative Component program runs under live and controlled
  bindings without topology-dependent application code.
- Structured traces contain Component identity and causation sufficient to
  explain execution without treating an incidental global sequence as semantic.
- Shutdown and cancellation accounting leaves no detached runtime-owned work.
- Backpressure, load, and fault behavior are characterized for the selected
  implementation.

## Migration and Compatibility

- Existing application code and tests must be audited for assumptions about a
  program-wide message order.
- Public APIs must not promise a single global mailbox or expose incidental
  global sequencing as a correctness mechanism.
- An existing single-loop implementation may remain temporarily or permanently
  if it satisfies this ADR. Its stronger incidental serialization is not a
  compatibility guarantee.

## Rollback

If implementation evidence shows that topology neutrality is impractical, a
future ADR may narrow the allowed implementation space or add an explicit
ordering contract. That decision must identify the user-visible value, coupling
cost, migration impact, and controlled-execution consequences; accidental
serialization alone is not sufficient evidence.
