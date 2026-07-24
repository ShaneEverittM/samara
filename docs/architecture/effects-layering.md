# Effect and Source Composition: Layers and Drivers

## Status

- Phase: Phase 6 live Driver implementation complete; audit ready.
- Date: July 23, 2026.
- API names are provisional; semantic roles follow `docs/glossary.md`.

## Purpose

Define how Samara composes EffectDescriptor and SourceDescriptor contracts before a
terminal boundary realizes them, while keeping application and Protocol policy separate
from runtime mechanism.

## Core Principles

- Descriptors are inert, typed declarations.
- Layers build composed descriptors inside the Samara program boundary.
- Drivers terminate descriptor stacks into the selected live surrounding world.
- Controlled execution supplies deterministic behavior for terminal descriptor contracts
  without silently invoking live Drivers.
- Runtime and Driver code own live, world-facing mechanism. Layers may own
  deterministic composition mechanism; Components and explicit Layers own
  application and Protocol policy.

*Adapter* is the conceptual umbrella for Layers and Drivers. It does not imply one
universal `Adapter` trait.

## Descriptor and Runtime Roles

### EffectDescriptor and EffectDriver

An `EffectDescriptor` describes one finite world-facing interaction. A Command combines
it with a pure one-shot message mapper.

```text
Command + EffectDescriptor
    -> zero or more Layers
    -> terminal EffectDescriptor
    -> live EffectDriver<D>, or controlled behavior
    -> EffectOutcome exactly once on normal completion
       (whole-scope abort may produce none)
    -> one-shot message mapper
    -> Component Message
```

Two identical-looking descriptors issued by different Commands are separate effect
invocations. EffectDescriptors therefore need not be comparable or cloneable.

### SourceDescriptor, Subscription, Source, and SourceDriver

A `SourceDescriptor` describes ongoing event production and is comparable for
Subscription reconciliation. A Subscription adds stable Component-local identity and a
reusable message mapper. A Source is the runtime-scoped realization.

```text
Subscription(identity + SourceDescriptor + message mapper)
    -> reconciliation
    -> SourcePlan(ordered Layers + terminal SourceDescriptor + mapper)
    -> live SourceDriver<D>, or controlled behavior
    -> Source
    -> SourceEvent zero or more times
    -> reusable message mapper
    -> Component Message
```

Stable identity belongs to the Subscription rather than the SourceDescriptor. The same
identity with an equal descriptor retains its Source and atomically adopts the latest
post-transition mapper. A changed descriptor atomically replaces it; removal cancels it.
Cancellation does not inherently manufacture a SourceEvent.

Each Source realization has a private runtime generation. After replacement commits, an
old-generation event or mapped Message that has not begun a Component transition is
discarded and traced. An already-running transition completes, and an old event is never
mapped through the new generation. Applications model deliberate overlap with separate
Subscription identities rather than depending on runtime generations.

The Effect and Source paths are intentionally parallel at the descriptor and Driver
boundaries. Their lifecycles remain asymmetric: an EffectOutcome is one terminal
completion, while a SourceEvent is one repeatable occurrence from ongoing work.

A **composed descriptor** contains one or more Layer applications around another
descriptor. A **terminal descriptor** is the innermost descriptor directly handled by a
live Driver or terminal controlled behavior. These modifiers apply to both EffectDescriptor
and SourceDescriptor; `TelemetryFeed` is merely an application alias for one composed
SourceDescriptor in the framed-socket reference example.

A **SourcePlan** is the runtime-owned compiled mechanism produced automatically during
reconciliation. It contains the terminal SourceDescriptor, its ordered Layers, and the
Subscription mapper. Applications declare the composed descriptor and bind or control
only the terminal descriptor; they never register its Layers redundantly. The exact Rust
representation remains implementation-selectable.

## Layer and Driver Roles

### Layer: Compositional Adapter

A Layer transforms a descriptor or event vocabulary into another declarative form. It:

- Runs with the same semantics in live and controlled execution.
- Performs no ambient I/O.
- May use deterministic runtime-scoped mechanism state, such as a framing buffer.
- Remains independently testable from the terminal world boundary.

For example, a length-delimited framing Layer may compose a byte SourceDescriptor into a
frame SourceDescriptor. The composed `Framed<Bytes, Codec>` value does not need its own
terminal Driver if the Layer can reduce it to the terminal byte descriptor and map the
resulting events.

The runtime lowers that composed value into a SourcePlan before profile-specific terminal
handling. Live and controlled execution use the same ordered Layer sequence and
state-transition semantics.

Samara expects more than one kind of Layer to emerge. This document names the category
without promising one universal `Layer` trait.

### Driver: Terminal Adapter

A Driver realizes a terminal descriptor against a live execution profile. It:

- May use Tokio, clocks, sockets, channels, and other world-facing resources.
- Runs only under a runtime-owned scope with supervision and cancellation.
- Implements mechanism rather than application or Protocol policy.
- Is supplied through program assembly, not associated with a Component implementation.

The provisional code-level roles are `EffectDriver<D>` and `SourceDriver<D>`. Their
generic descriptor parameter supplies the static relationship without forcing Driver
wiring into the descriptor or Component types.

Controlled execution need not implement or call those live Driver traits. The exact Rust
shape of live and controlled profile bindings remains open.

Under ADR-0004, a normally returning EffectDriver result becomes exactly one
Succeeded or Failed EffectOutcome and invokes its mapper once. Whole-scope Cancel or
runtime-fault cleanup is an abort instead: it cancels the Driver future without
manufacturing an outcome for an application that is ending. Explicitly accepted
per-effect cancellation outcomes remain ordinary exactly-once typed outcomes.

For a live Source, successful sink calls preserve acceptance order. The first accepted
`end`, accepted `fail`, or active silent SourceDriver return wins one terminal event.
Cancellation, replacement, shutdown, or runtime fault winning first withdraws the
generation without an event; later sink calls receive `DriverStopped`. Normal Source
ending does not imply automatic restart while the same Subscription desire remains.

## Mechanism and Policy

Mechanism examples:

- Logical and wall-clock scheduling.
- Socket connect, read, and write primitives.
- Tokio channel receipt.
- Task lifecycle, cancellation, delivery, and supervision.

Policy examples:

- Retry and backoff rules.
- Reconnect strategy.
- Choice of framing or wire Protocol.
- Domain timeout meaning and error mapping.

A Layer may implement the selected framing mechanism, but the decision to use that
framing and the application's response to its events remain explicit in declarations and
Component logic. A Driver must not hide retry, reconnect, or domain interpretation behind
an apparently primitive operation.

## Representative Compositions

These examples are schematic, not frozen Rust APIs.

```text
Command::effect(
    PersistFrame { frame },             // EffectDescriptor
    TelemetryMessage::Persisted,        // one-shot message mapper
)
```

Live execution terminates `PersistFrame` through its `EffectDriver`; controlled execution
provides the typed EffectOutcome directly.

```text
Subscription(
    identity = "socket-frames",
    descriptor = Framed(TcpBytes { endpoint }, U16LengthDelimited),
    map = TelemetryMessage::Socket,
)
```

`Framed` is a Source Layer. `TcpBytes` is the terminal SourceDescriptor. Live execution
uses `SourceDriver<TcpBytes>`; controlled execution scripts the same terminal descriptor
contract. Both profiles automatically run the same framing Layer and message mapper; the
application binds neither `Framed` nor its Layer separately.

## Contract Rules

- Runtime APIs must not encode Protocol-specific policy.
- Layers must be deterministic for equivalent inputs and contain no ambient I/O.
- Drivers must remain terminal, narrow, runtime-scoped, and profile-selected.
- All world-facing side effects must cross a declared terminal descriptor boundary.
- Behaviorally relevant EffectOutcomes and SourceEvents return through message mappers.
- Missing controlled behavior fails explicitly rather than falling through to a live
  Driver. Reaching an unhandled terminal descriptor faults the controlled run before a
  message mapper is invoked; state and trace remain inspectable and cancellation remains
  available.
- Retained Sources install the latest projected mapper without restarting.
- Replaced Source generations cannot deliver stale work into a transition or through the
  replacement mapper.
- Composed SourceDescriptors lower automatically to SourcePlans; profile assembly binds
  terminal descriptors only.
- Adding a new built-in terminal descriptor or Driver must be justified as reusable
  mechanism rather than hidden application policy.
- A Driver panic or unavailable terminal live mechanism faults and closes the live scope;
  it does not invent typed application Error data.
- v0 internal live delivery is unbounded after successful admission and does not
  intentionally drop accepted work due to capacity. This is a documented memory-growth
  limitation, not an overload or backpressure promise.

ADR-0004 resolves the Phase 6 framing gate by using `bytes::Bytes` for
first-party TCP chunks and requiring pure Decoder finalization. On normal inner `Ended`,
Framed finalizes exactly once, emits any final frames in order, then emits `Ended`; a
finalization Error emits one decode failure and no `Ended`. Underlying failure, earlier
decode failure, replacement, cancellation, shutdown, and runtime fault do not invoke
finalization. The exact method spelling remains implementation-reviewable.

## Historical Note

Earlier documents and proof-of-concept code used *effect handler*, *interpreter*,
*backend*, and *adapter* interchangeably and sometimes called inert descriptor values
`Effect` or `Source`. They also used “Layer 1/2/3” for architectural strata. The current
canonical vocabulary reserves `EffectDescriptor` and `SourceDescriptor` for inert values,
`Source` for the runtime-scoped ongoing realization, `Layer` for compositional adapters,
and `Driver` for terminal live-world adapters.
