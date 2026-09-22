# ADR 0003: Controlled Execution Semantics

- Status: Accepted
- Date: July 23, 2026
- Decision owners: Samara maintainers
- Extends: [ADR 0002](0002-runtime-topology-and-ordering.md)

> Supersession note (July 25, 2026): [ADR-0008](0008-closed-program-capabilities.md)
> supersedes this ADR's open Effect/Source dependency qualification and normal
> runtime discovery of missing controlled behavior. The scheduling, trace,
> Source-cutover, and work-accounting decisions below remain in force.

> Extension note (September 22, 2026): [ADR-0011](0011-request-timeouts.md)
> adds opt-in Request timeouts and harmless late Replies. Successful-only and
> deferred-timeout statements below describe the original unbounded forms.

## Context

ADR-0002 requires deterministic controlled execution without making its chosen
schedule a live ordering guarantee. Phase 5 cannot implement that requirement
until Source reconciliation, composed descriptor interpretation, equal-time
scheduling, structural tracing, program validation, and work accounting have
precise observable contracts.

The Phase 4 declarative-work kernel deliberately left these policies open. A
contract workshop resolved the load-bearing choices below before controlled
runtime implementation begins.

## Decision Drivers

- Make the current `subscriptions()` projection authoritative without
  restarting unchanged world resources.
- Prevent replaced Sources from leaking stale events across a lifecycle
  boundary.
- Run the same Layers in live and controlled profiles while binding only true
  terminal descriptors.
- Make controlled schedules reproducible without implying cross-profile order.
- Explain runtime behavior without imposing domain tracing boilerplate.
- Reject invalid explicit assembly as early as the available information
  permits.
- Account for semantic obligations rather than scheduler machinery.
- Implement one useful correlated request path without prematurely selecting
  its full failure and abandonment policy.

## Decision

### Retained Sources Adopt the Latest Mapper

`ComponentId` plus `SubscriptionId` identifies one ongoing desire, and
`SourceDescriptor` equality determines whether its Source realization can be
retained. The message mapper is not part of either identity.

When post-transition reconciliation finds the same identity and an equal
descriptor, the runtime retains the existing Source and atomically installs
the mapper from the latest `subscriptions()` projection. Messages already
created remain unchanged. SourceEvents mapped after reconciliation use the
latest mapper.

### Replacement Is a Hard Generation Cutover

Every Source realization has a private runtime generation. Replacing a changed
descriptor atomically withdraws the old generation and activates a new one.

An old-generation SourceEvent or mapped Message that has not begun a Component
transition when replacement commits is stale. The runtime discards it and
records the discard in the structural trace. An already-running transition
finishes because transitions for one Component do not overlap. An old event is
never mapped through the new generation's mapper.

Applications that require overlap or draining model the lifetimes as separate
Subscription identities. Applications that need generation as domain data
carry their own domain generation rather than observing the runtime's private
generation.

### Composed Sources Lower to a Runtime-Owned SourcePlan

During reconciliation, the runtime automatically lowers a composed
SourceDescriptor such as `Framed<TcpBytes, D>` into a `SourcePlan`. The plan
contains the terminal SourceDescriptor, its ordered profile-independent
Layers, and the Subscription's message mapper. This is a compiled runtime
mechanism, not another application declaration or lifecycle identity.

Applications declare the composed descriptor and bind or control only its
terminal descriptor. They do not register the composed descriptor or its
Layers redundantly. Live and controlled execution apply the same ordered
Layers. This ADR governs that application-facing behavior while leaving the
exact Rust representation of `SourcePlan` and general Layer lowering open to
implementation evidence.

### Equal-Time Work Uses Deterministic Causal Insertion Order

The controlled scheduler orders runnable work by:

```text
(logical deadline, deterministic insertion ticket)
```

Tickets are assigned by these v0 rules:

- Commands emitted by one transition follow declaration traversal order.
- Controlled inputs follow the order supplied by the harness.
- Initial Component work is canonicalized by `ComponentId`, never registration
  order.
- Work emitted while handling another event receives its tickets after its
  cause.

This tie-break is part of the v0 controlled-runtime semantics. It is
reproducibility machinery, not a live or domain ordering guarantee.
Applications that require the same sequence across profiles express an
explicit causal dependency.

### Controlled Trace Is Always-On and Structural in v0

Controlled execution always collects an in-memory structural trace. Tests read
it after driving the runtime; Phase 5 does not require a streaming callback or
observer attachment seam.

Every entry has a common envelope equivalent to:

```text
TraceRecord {
    id: TraceId,
    at: LogicalTime,
    cause: Option<TraceId>,
    event: TraceEvent,
}
```

Initialization and controlled harness inputs are roots with no parent. Every
other record has exactly one immediate causal parent. The minimum structural
events explain:

- Component transitions;
- Command kinds, concrete descriptor types, and targets;
- desired Subscriptions and Source lifecycle, including stale-generation
  drops;
- EffectOutcome and SourceEvent terminal shapes;
- logical time and immediate causation.

The generic runtime trace is not required to copy domain payload fields from
descriptors or Messages. Direct Component and typed-intent tests verify those
values. Descriptor/message payload capture and typed semantic trace
projections are stretch goals, not Phase 5 requirements.

Trace compatibility follows ordinary crate API versioning. v0 defines no
durable trace schema, storage format, or replay promise.

### ProgramBuilder Fails on Explicitly Knowable Assembly Errors

`ProgramBuilder::build()` is fallible. It validates only facts explicitly
represented by assembly:

- every `ComponentId` is unique;
- every `(Protocol type, PortId)` declaration is unique;
- every declared Port is bound exactly once; and
- every bound provider was registered in the same builder.

Port cycles are legal and are not detected. The builder does not introspect
arbitrary Component fields or behavior-dependent `Command::send` edges, so it
does not claim to construct or validate a closed static dependency graph. A
send to a Component absent from the running Program produces a runtime
diagnostic when interpreted.

### Missing Controlled Behavior Faults at the Terminal Boundary

If interpretation reaches a terminal descriptor with no controlled behavior,
the drive operation fails at that boundary. It invokes no live Driver and no
message mapper. The runtime records a diagnostic identifying the Component,
work occurrence, and descriptor type, then becomes faulted.

Models and trace remain inspectable and controlled cancellation remains
available, but the faulted run cannot resume driving. The test must correct its
controlled-world setup and start a fresh run. The exact taxonomy and
ergonomics of harness calls that fail before entering program execution remain
implementation and audit details.

### An Effect May Deliberately Discard Its Outcome

`Command::effect_discarding_outcome(&capability, descriptor)` declares the same
finite typed effect obligation as `Command::effect(&capability, descriptor)`,
but deliberately installs no application continuation. This is one-way intent
from the Component, not detached work.

Controlled execution still exposes the descriptor through `next_effect`,
counts it as pending work, and requires the harness either to complete it with
one typed `EffectOutcome` or to cancel the controlled scope. Supplying an
outcome removes the obligation and records the ordinary structural
`EffectOutcome` trace entry, but schedules no Message and therefore causes no
Component transition. Supplying `EffectOutcome::Cancelled` follows the same
rule. Whole-scope controlled cancellation continues to clear the obligation
without manufacturing an outcome.

Missing controlled behavior remains a runtime fault even when the outcome
would have been discarded. Discarding an application continuation does not
discard Driver or runtime diagnostics.

### Pending Work Counts Semantic Obligations

Work accounting does not count runtime implementation machinery separately.

- `pending_now` counts accepted Component Messages and due timers ready to run.
- `pending_later` counts pending effects, future timers, active Sources (one per
  Source regardless of possible future event count), and outstanding Requests.
- Transitions, interpreters, trace records, tasks, locks, and internal queues
  are not additional obligations.

Request delivery may temporarily create both a queued-Message obligation and
an outstanding-Request obligation. A successful `run_until_idle()` normally
returns with `pending_now == 0` while `pending_later` may remain nonzero.
Controlled cancellation reduces both counts to zero.

### Phase 5 Implements Successful Request/Reply Only

Phase 5 implements typed Request delivery through a bound Port, one opaque
transport-correlation token per occurrence, an at-most-once Reply, mapping to
`RequestOutcome::Replied`, causal tracing, and one outstanding-Request
obligation.

Phase 5 does not generate `Failed`, `TimedOut`, or `Cancelled` request
outcomes. An unanswered Request remains pending until controlled cancellation
cleans up runtime ownership. Abandonment diagnostics, deadlines, failure
taxonomy, late Replies, delegation, and in-band cancellation policy remain
deferred.

## Controlled Equivalence in v0

Two identical controlled runs must produce equivalent final Component state
and equivalent structural semantic traces under the scheduling rules above.
The generic trace distinguishes runtime structure and concrete descriptor
types, but does not claim to distinguish two same-typed descriptors solely by
their domain payloads. Direct typed-intent tests remain required evidence that
Components emitted the intended descriptor values.

## Consequences

### Positive

- Reprojecting Subscriptions updates application meaning without unnecessary
  live-resource churn.
- Source replacement has a precise, topology-neutral stale-event boundary.
- Applications configure one composed Source path for both execution profiles.
- Equal-time controlled runs are reproducible without blessing their order in
  live execution.
- Useful causal traces require no descriptor-specific author boilerplate.
- Invalid explicit graph assembly fails before runtime startup.
- Quiescence and cancellation reports describe application obligations rather
  than an implementation's task or queue shape.

### Negative

- Source delivery must carry private generation metadata until a transition
  begins.
- The runtime must preserve deterministic declaration traversal and insertion
  ticket allocation.
- Structural traces alone cannot compare same-typed domain payloads.
- Some graph errors and behavior-dependent missing targets remain runtime
  diagnostics.
- The initial Request path intentionally leaves several terminal policies
  unresolved.

## Required Evidence

Phase 5 acceptance must demonstrate:

- retained Sources use the newest mapper without restarting;
- replaced generations cannot deliver stale events or Messages;
- composed Sources lower through ordered Layers to controlled terminal
  behavior, with no live Driver fallback;
- repeated controlled runs use the specified equal-time rule and yield equal
  structural traces and final state;
- every trace record has logical time, every root has no parent, and every
  non-root record has exactly one immediate causal parent;
- invalid explicitly knowable Program assembly fails during `build()`;
- missing controlled behavior faults at its terminal boundary while preserving
  inspectability and cancellation;
- pending-work accounting follows the semantic units above and reaches zero on
  cancellation; and
- successful typed Request/Reply preserves correlation, at-most-once reply,
  mapping, causation, and ownership.

Direct tests must separately compare typed descriptor intent where domain
payload equality matters.

## Deferred Decisions

- Domain-payload-complete trace capture, typed trace projections, streaming
  observers, durable trace storage, and replay.
- Request failure, timeout, abandonment, late-Reply, delegation, and in-band
  cancellation semantics.
- Notification delivery-failure behavior.
- Shutdown drain-versus-cancel and live backpressure/overload policy.
- The exact general Rust abstractions for Layer lowering and profile bindings.
- `bytes` adoption and Decoder EOF/finalization semantics. The Phase 4
  limitation remains unchanged and must be resolved before live TCP framing in
  Phase 6.

## Rollback

Changing equal-time ordering, Source cutover, trace causation, or pending-work
semantics changes controlled observability and requires a superseding ADR plus
updated conformance evidence. Internal data structures may change without an
ADR when the behavior above remains unchanged.
