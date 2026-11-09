# Phase Audit History

Each implementation phase in `docs/goal-mode-checklist.md` ended with an audit
packet that Shane accepted. The packets are collapsed here to one entry each,
November 8, 2026; the full packets remain in git history before that date.
Evidence for every accepted scenario lives in
[`docs/testing/v0-acceptance-matrix.md`](../testing/v0-acceptance-matrix.md),
and the governing decisions in `docs/adr/`.

## Phase 2: Executable Acceptance Contract

Accepted July 22, 2026 at commit `8408509`. The root crate exposed the
compiler-checked candidate API, the three reference Components compiled
against it, and V1-V11 mapped to named acceptance scenarios. No runtime
behavior existed; placeholder live and controlled methods returned honest
errors. The decision gates it left open (retained-Source mapper replacement,
semantic trace, fallible assembly validation, request lifecycle, bindings,
shutdown) were each closed by a later phase or ADR.

## Phase 3: Component Kernel

Accepted July 22, 2026 at commit `ba0d90f`. An assembled Program owned each
Component's immutable configuration, exclusively owned Model, and optional
startup Command. Exclusive mutable access to one private kernel made
overlapping transitions impossible without a public mailbox, task, queue, or
lock contract. No Command was interpreted and no Subscription reconciled.

## Phase 4: Declarative Work Kernel

Accepted July 22, 2026 at commit `c629c4c`. Commands flattened into inert
declarations; effect declarations could be intercepted as typed
`EffectInvocation` values without executing world work. Each kernel owned
descriptor-only Subscription bookkeeping (start, retain, replace, cancel under
Component-local identity). `FramedLayer` was the first compositional Source
Layer. No terminal live or controlled behavior existed.

## Phase 5: Controlled Execution

Accepted July 23, 2026 at commit `8158f1b`. One topology-neutral Program could
be assembled into deterministic, synchronously driven controlled execution:
typed Commands interpreted, Messages routed through transitions, Sources
reconciled and lowered through ordered Layers, effects intercepted, logical
time advanced, successful Requests completed, pending work reported, and an
always-on structural causal trace collected. No live Driver, Tokio I/O,
wall-clock sleep, or detached task was reachable from the controlled path.
ADR-0008 later added Program-issued Effect and Source capabilities and full
profile-build validation; the packet's API spellings predate that.

## Phase 6: Live Tokio Runtime

Accepted September 22, 2026 (packet dated July 23, 2026). The same Program,
Component, Command, descriptor, Layer, and Port contracts ran against live
terminal Drivers inside one structured runtime scope, with atomic admission
closure at shutdown, per-Component serialization, and no program-wide order
for independent live events. One runtime-owned dispatcher and unbounded
internal delivery were implementation choices, not contract. First live
bridges: exact one-shot `mpsc` bindings and one-connection TCP byte Sources
with explicit `Decoder::finish`.

Deferrals recorded at acceptance, with what has since landed:

- Shutdown deadlines, grace, and escalation: landed as ADR-0010.
- Request timeouts and harmless late replies: landed as ADR-0011. Per-request
  cancellation, abandonment, and delegation remain deferred.
- Bounded queues, quotas, shedding, fairness, overload metrics, and stable
  throughput targets: deferred; see the bounded-work item in
  [`docs/notes/framework-follow-ups.md`](../notes/framework-follow-ups.md)
  and the 0.2 stage of the release plan.
- Notification delivery-failure policy and per-effect deadline or
  supersession policy: deferred.
- Automatic Source retry or restart, restartable bridges, DNS, connection
  policy, Driver isolation and recovery: deferred; the Subscription restart
  revision follow-up covers the first of these.
- Public live tracing, payload capture, typed trace projections, durable
  storage, persistence, and replay: deferred; persistence and replay are out
  of v0 scope by AGENTS.md.
