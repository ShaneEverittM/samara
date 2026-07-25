# ADR 0004: Initial Live Runtime Semantics

- Status: Accepted
- Date: July 23, 2026
- Decision owners: Samara maintainers
- Extends: [ADR 0002](0002-runtime-topology-and-ordering.md) and
  [ADR 0003](0003-controlled-execution-semantics.md)

## Context

ADR-0002 defines topology-neutral ordering and ADR-0003 defines controlled
execution, but Phase 6 cannot implement the live Tokio profile until its
admission, shutdown, Driver completion, overload, framing, and first-party
bridge behavior is explicit.

These areas eventually need product-level policies informed by real workloads.
They do not all need mature configurability in the first live runtime. This ADR
therefore adopts one deliberately simple v0 mechanism that is observable and
testable without presenting it as Samara's final overload, recovery, or
shutdown product policy.

This ADR is the accepted Phase 6 implementation contract. Its first-cut
mechanics are active while the product-level policies explicitly left
unresolved below remain provisional.

## Decision Drivers

- Preserve structured ownership without requiring every open Source to finish
  naturally before shutdown.
- Give successful ingress and Driver calls an unambiguous acceptance meaning.
- Avoid inventing application data when the whole runtime scope is ending or a
  Driver violates its contract.
- Preserve the same Source Layers and decoder semantics across live and
  controlled execution.
- Ship useful first-party Tokio `mpsc` and TCP paths without embedding retry,
  reconnect, or framing policy in terminal Drivers.
- Characterize the simple implementation before selecting bounded queues,
  shedding, fairness, or overload controls.
- Reaffirm causal partial order rather than accidentally blessing one live
  scheduler sequence.

## Decision

### External Ingress Has an Admission Boundary

Starting either shutdown mode closes external ingress. A
`ComponentHandle::send` that succeeds has been accepted for runtime-managed
delivery; it does not mean the target transition has completed. A send after
ingress closes fails explicitly.

A send racing shutdown resolves at one admission boundary: it is either
accepted and covered by the selected shutdown policy or rejected. Once a send
returns success, the runtime must not silently discard it as unadmitted work.

### `Shutdown::Drain` Stops Sources, Then Drains Accepted and Causal Work

Drain has one atomic cutoff: it closes external handles, disables every new or
restarted Source realization, and stops or cancels all active Sources. Open or
infinite Sources therefore do not, merely by remaining open, prevent drain from
finishing. Removing these Sources during shutdown does not synthesize a
SourceEvent.

Drain retains and processes:

- Component Messages accepted before ingress closed;
- Source item or terminal deliveries accepted by an active generation before
  the cutoff, whether they have already been mapped into Messages or are still
  queued for ordered mapping;
- due and future timers already scheduled;
- already-issued finite effects and Requests and their eventual results; and
- finite work and timers causally emitted while processing the retained work.

Newly desired or previously terminal Sources are not realized or restarted
after drain starts. Finite effects, Requests, Messages, and timers emitted by a
draining transition remain eligible work, recursively; otherwise a transition's
declared consequences could be silently lost. Drain does not cancel eligible
finite effects merely to finish sooner.

Drain has no implicit deadline. It may wait forever for a hung Driver, an
unanswered Request, a far-future or recurring timer, or application behavior
that keeps producing finite work. Callers that require prompt termination use
`Shutdown::Cancel` or apply their own deadline before choosing it.

### `Shutdown::Cancel` Ends Application Driving and Cleans Up Ownership

Cancel closes ingress, stops application driving, cancels queued and deferred
semantic obligations, cancels runtime-owned Driver futures and Source tasks,
then joins or aborts every runtime-owned task. It does not detach work.

Cancel may discard work that Drain would retain; this is the documented scope
abort boundary, not an internal overload drop. Whole-scope cancellation does
not manufacture Component Messages,
EffectOutcomes, SourceEvents, or RequestOutcomes merely because the application
is ending. In particular, canceling an in-flight effect as part of runtime
shutdown drops or cancels its live future and does not invoke its message
mapper. This is distinct from an explicit per-effect cancellation outcome
defined by an effect contract or supplied by controlled behavior.

Future per-effect deadlines, supersession, and in-band cancellation remain
deferred.

### Successful Scope Closure Has Zero Remaining Work

When either shutdown mode returns success, all runtime-owned tasks and semantic
obligations are closed:

```text
remaining == pending_now == pending_later == 0
```

`ShutdownReport::completed` and `cancelled` refer to ADR-0003 semantic
obligations rather than tasks, queues, locks, or interpreter steps. Their exact
aggregation and values remain provisional diagnostics, so v0 conformance tests
must not depend on exact counts.

### Effect Drivers Complete Once or Are Scope-Cancelled

A normally returning `EffectDriver` result maps exactly once:

- `Ok(output)` becomes `EffectOutcome::Succeeded(output)`; and
- `Err(error)` becomes `EffectOutcome::Failed(error)`.

For `Command::effect`, the originating one-shot mapper is then invoked exactly
once and its Message re-enters runtime-managed delivery. For
`Command::effect_discarding_outcome`, the runtime accepts the same terminal
outcome and closes the same finite obligation, but deliberately schedules no
Message.

The discarded-outcome mode is not detached execution. Drain waits for it and
retains finite work causally emitted before it just as for any other effect.
Cancel aborts its Driver future and joins or aborts the runtime-owned task.
Missing bindings, Driver panics, and runtime mechanism faults still surface to
the host. Whole-scope shutdown cancellation manufactures no outcome in either
mode. Completion racing a scope abort resolves at one boundary: a Driver result
accepted first completes the obligation according to its declared continuation
mode, while an abort that wins first cancels the future without an outcome.

### Source Terminal State Is Exactly Once

`SourceSink::end` and `SourceSink::fail` are explicit terminal operations.
Successful calls from one Source generation preserve acceptance order. Success
means accepted for runtime delivery, not that the mapped Component transition
has completed.

The first accepted terminal operation while the generation is active wins,
later sink use returns `DriverStopped`, and the SourceDriver future returning
afterward creates no duplicate event.

If a `SourceDriver` future returns while its generation remains active and no
terminal sink call was accepted first, the runtime treats that return as one
normal `SourceEvent::Ended`. This gives silent normal return a deterministic
meaning without requiring every simple Driver to duplicate a final `end` call.

Removing or replacing a Subscription, shutting down its runtime, or faulting
the scope may instead win terminal arbitration. That path stops and joins or
aborts the Source task without synthesizing a SourceEvent, and later calls
return `DriverStopped`. The private generation checks from ADR-0003 reject late
or stale events that race that cutover. A normally ended or failed Source stays
inactive while the same desire remains; v0 does not automatically restart it.

### Driver Panics Are Runtime Faults

An EffectDriver, SourceDriver, or runtime-owned Driver task panic is not typed
application Error data. It faults the runtime, initiates cancellation and
structured cleanup, and is surfaced to the host as `RuntimeError`. The runtime
does not invent an EffectOutcome or SourceEvent whose typed Error payload it
cannot truthfully construct.

The exact containment granularity, panic payload reporting, and any future
recovery or restart policy remain provisional beyond this boundary.

### v0 Uses Unbounded Internal Delivery While the Scope Is Healthy

While a healthy runtime scope is running, the initial live runtime uses
unbounded runtime-internal delivery after admission. A successful
`ComponentHandle::send` or `SourceSink` method means that value was accepted or
enqueued by the runtime. A closed, cancelled, or faulted runtime returns an
error instead. Accepted work is not intentionally dropped because an internal
capacity was reached. The documented Cancel and runtime-fault cutovers may
discard queued application work as part of aborting the scope.

v0 provides no configurable internal capacity and no internal overload or
backpressure guarantee. Sustained production faster than Component processing
may grow memory without bound, and memory exhaustion is unsupported. Phase 6
load work characterizes this behavior; it establishes no stable throughput,
capacity, latency, or fairness promise.

The first-party `mpsc` bridge preserves the pressure behavior of the upstream
Tokio channel chosen by the application until the bridge receives an item.
Once Samara accepts that item, delivery follows the unbounded internal rule; the
bridge does not add a second bounded-pressure contract.

### First-Party TCP Uses `bytes` and No Hidden Policy

First-party TCP remains in Phase 6. One Source realization owns one TCP
connection and emits `bytes::Bytes` chunks. Decoder state may use
`bytes::BytesMut`.

The TCP Source Driver:

- connects once per Source realization;
- maps connection and read errors to its typed Source failure;
- maps peer EOF to normal `SourceEvent::Ended`;
- closes its socket when cancelled; and
- performs no retry, reconnect, framing, or domain interpretation.

Framing belongs in a Layer. Retry and reconnect belong in an explicit Layer or
application logic. Exact public module and concrete type names remain subject
to implementation review unless already frozen elsewhere.

### Decoder Finalization Is Explicit

The pure `Decoder` contract gains a required EOF/finalization operation,
spelled `finish` or an implementation-reviewed equivalent. It accepts the
runtime-owned decoder state and returns zero or more final frames or the
Decoder's typed Error.

When an underlying Source ends normally, `Framed` finalizes exactly once:

1. On success, it emits all final frames in order and then emits `Ended`.
2. On finalization failure, it emits one
   `SourceEvent::Failed(FramedError::Decode(error))` and does not emit `Ended`.

An underlying Source failure remains
`SourceEvent::Failed(FramedError::Source(error))`. It is not decoder EOF and
does not run finalization as if the byte stream ended normally. An earlier
decoder failure, replacement, cancellation, shutdown, and runtime fault likewise
do not invoke finalization. Successful calls from one Source preserve order, so
finalization cannot overtake an earlier accepted chunk. No normal EOF path may
silently discard an incomplete decoder buffer; accepting or rejecting such
input is an explicit Decoder decision.

### The First-Party `mpsc` Binding Is One-Shot

One `tokio::sync::mpsc::Receiver<T>` backs at most one active realization of
its `StreamDescriptor<T>`. Channel closure maps to one normal `Ended` event.

The receiver is a unique live resource and is consumed by the first activation
of its exact binding. A second concurrent claimant, or any later activation
after normal end or cancellation, faults the live runtime rather than hanging,
silently producing no events, or pretending a new receiver exists.
`StreamDescriptor<T>::Error` is `Infallible`, so this exhausted-resource misuse
cannot truthfully be represented as an in-band Source failure. Exact public
module organization remains an implementation-review detail.

### Live Binding and Runtime Faults Close the Scope

Missing, duplicate, or ambiguous live bindings that assembly can know make
`LiveRuntimeBuilder::build()` fail. A missing terminal binding discovered only
when dynamic work reaches it, an exhausted one-shot `mpsc` binding, a Driver
panic, or an equivalent live mechanism violation faults the running scope.

A live runtime fault closes ingress, stops application driving, cancels queued
obligations and Driver tasks, and joins or aborts every runtime-owned task.
Aborted effects, Sources, and Requests do not invoke their application mappers.
The fault is surfaced as `RuntimeError` through later
`ComponentHandle::send` calls and the owning `RuntimeTask::shutdown` boundary
after structured cleanup. Exact error variants, panic payloads,
isolation, restart, and recovery remain provisional.

### A Public Live Observer Is Deferred Beyond v0

Phase 6 does not add a public live trace, callback, or observer API. Internal
test instrumentation may observe scheduling and lifecycle behavior, but it is
not an application-facing behavior surface and Components cannot read it.

The acceptance scenario `v10_live_observer_has_no_feedback_path` is deferred
beyond v0 rather than required for Phase 6 or final v0 conformance. Controlled
execution's accepted read-afterward structural trace contract is unchanged.

### Live Conformance Uses Causal Partial Order

Live execution preserves per-Component non-overlap, command-to-outcome,
send-to-delivery, request-to-reply, and any explicitly promised FIFO or
sequencing edges. It does not provide a relative order for independent
completions.

Phase 6 tests must exercise or deliberately force both valid orders of
independent live completions and compare their causal partial order. They must
not compare a live trace vector against ADR-0003's controlled insertion-ticket
order or treat one observed interleaving as a global-order promise.

## Consequences

### Positive

- Shutdown has a usable first cut with explicit admission and structured
  ownership.
- Drain can finish for finite work even when the program normally owns ongoing
  Sources.
- Scope cancellation does not inject misleading application data into an
  application that is no longer running.
- Driver return, terminal calls, EOF, and panic each have one observable
  interpretation.
- TCP framing behavior remains shared between live and controlled profiles.
- The simplest delivery mechanism can be measured before a bounded overload
  design is selected.

### Negative

- Drain is not time-bounded and can wait forever.
- Unbounded internal delivery can exhaust memory under sustained overload.
- A one-shot `mpsc` receiver cannot transparently survive Subscription
  reactivation.
- Runtime shutdown cancellation is not delivered as an application Message,
  so it cannot be used for domain cleanup inside a Component.
- Driver panic faults the whole runtime in v0 rather than isolating one
  Component or automatically restarting work.
- No public live trace API ships in v0.

## Required Evidence

Before Phase 6 can be accepted, executable evidence must demonstrate:

- successful ingress is delivered, ingress after shutdown starts is rejected,
  and concurrent ingress never overlaps one Component's transitions;
- Drain stops Sources, realizes no new Sources, and processes accepted Messages,
  pre-cutoff Source deliveries, and causally emitted finite work through clean
  scope closure;
- Cancel invokes no effect, Source, or Request mapper solely because the scope
  ended and leaves no task or semantic obligation owned;
- successful shutdown reports zero `remaining`, `pending_now`, and
  `pending_later` without asserting exact completed/cancelled counts;
- EffectDriver success and failure complete each effect exactly once: mapped
  effects invoke their mapper once, discarded-outcome effects schedule no
  Message, and scope cancellation does neither;
- explicit Source terminal calls and implicit normal return each terminate
  exactly once, and cancellation/removal/replacement produce no unpromised
  SourceEvent;
- stale Source generations cannot deliver after live replacement;
- knowable live binding errors fail assembly, while a dynamic missing binding,
  Driver panic, or exhausted one-shot resource faults the runtime and still
  closes all runtime-owned work;
- accepted internal delivery does not silently drop under a bounded
  characterization workload, while the results are reported as non-normative;
- TCP emits `Bytes`, maps connect/read/peer-EOF correctly, contains no hidden
  retry or framing, and closes on cancellation;
- Framed finalization emits final frames before `Ended`, maps finalization
  failure without an additional `Ended`, and does not treat underlying failure
  as EOF;
- one-shot `mpsc` closure ends normally and duplicate activation or
  reactivation faults the scope explicitly;
- both reference Components run unchanged in live and controlled profiles with
  equivalent typed boundary observations; and
- independent live completions are accepted in either order while all causal
  edges remain intact.

## Explicitly Unresolved Beyond This First Cut

This ADR does not settle:

- bounded internal queues, admission quotas, load shedding, coalescing,
  fairness, priorities, queue metrics, or a stable overload API;
- shutdown deadlines, grace periods, escalation, service readiness, or
  host-signal policy;
- exact diagnostic counts in `ShutdownReport`;
- per-effect deadline, supersession, abandonment, or in-band cancellation;
- Request failure, timeout, cancellation, abandonment, late Reply, or
  delegation;
- Notification delivery-failure semantics;
- Driver panic isolation, restart, or recovery after the required runtime
  fault boundary;
- automatic Source retry or restart;
- restartable, shareable, broadcast, or multi-consumer channel bridges;
- DNS or TLS configuration for the first-party TCP Source, write-side TCP
  effects, socket tuning, reconnect, or protocol framing policy (the separate
  raw HTTP Effect is governed by ADR-0005);
- a public live observer, payload-complete trace, durable storage, or replay;
  and
- the exact general Rust abstraction for live profile bindings and downstream
  Layers, or exact first-party module/type naming not already frozen.

## Rollback

Changing admission success, shutdown work eligibility, Driver terminal
behavior, Decoder EOF semantics, accepted-delivery loss behavior, or
first-party bridge lifecycles now requires a superseding ADR and updated
conformance evidence. Internal task, queue, and supervision structures may
change without an ADR when the behavior above remains unchanged.
