# Samara API Guidance

- Status: Draft
- Date: July 25, 2026
- Scope: Consumer-facing API design rationale

This document is a durable FAQ for people designing and implementing Samara's
API. It records why important boundaries exist so implementation work does not
re-litigate them accidentally. It guides the shape of the API without freezing
the exact Rust syntax or every v0 policy. Terms follow the canonical usage in
[`glossary.md`](glossary.md).

## What Are We Optimizing For?

Samara code should be readable without being unnecessarily terse, and ergonomic
without being magic. Removing ceremony is valuable when it preserves the
explanatory path from input, through state transition and EffectDescriptor, back to
an outcome message. Hiding semantically necessary information is not an
ergonomic improvement.

## Does the API Let the Programmer Directly Express Intent?

This is Samara's primary API review question:

> Does this API let the programmer state the relevant intent directly, exactly
> once, while leaving incidental mechanism to Samara?

Direct does not necessarily mean brief. An API is indirect when users must
spell out runtime machinery to express an application decision, but it is also
indirect when hidden behavior forces readers to infer semantically important
facts. Good Samara APIs name the state, event, effect, ongoing source, or
Component interaction that matters while keeping tasks, channels, correlation
tables, and other execution machinery behind the runtime boundary.

## When Should Samara Reuse Built-In Rust Traits and Patterns?

Lean on built-in Rust traits, naming conventions, and control-flow patterns
when their established meaning matches Samara's semantic contract. Do not
invent a Samara-specific conversion trait when `From`, `Into`, `Iterator`, or
another standard abstraction already says the same thing. Familiar Rust makes
the application easier to read, improves compiler diagnostics, and reduces the
amount of Samara-specific vocabulary a user must learn.

This is not a mandate to force unlike concepts into superficial symmetry. A
standard trait should be used only when its full contract fits. In particular,
`From<BoundaryValue> for Message` is a natural spelling for one total,
canonical conversion from an `EffectOutcome`, `RequestOutcome`, or
`SourceEvent` into a Component Message. Following standard Rust guidance,
applications implement `From`; the standard library supplies the reciprocal
`Into` implementation automatically.

Continuation-bearing APIs therefore use a paired convention:

- the short method uses `Message: From<BoundaryValue>` as the default
  continuation; and
- a `_with` method accepts an explicit mapper when the call site must capture
  context or distinguish this occurrence from another occurrence with the same
  boundary type.

For example, `Command::effect(effect)` uses `Message::from`, while
`Command::effect_with(effect, mapper)` preserves an explicit continuation.
The same convention applies to Requests, Subscriptions, and pure HTTP response
pipelines where their lifecycle semantics permit it.

The Component method's concrete `Command<Message>` or
`Subscriptions<Message>` return context normally lets Rust infer the
destination Message type. An isolated local expression may need an ordinary
type annotation. This is compile-time trait selection, not runtime detection.
As with an explicit closure, implementing `From` does not mechanically enforce
purity or determinism; that remains part of Component conformance.

`From` expresses one canonical meaning for one source and destination type. It
cannot and should not erase the irreducible distinction between two physical
call sites that interpret the same outcome differently. Those sites use the
explicit `_with` form. Samara also keeps
`Component::update(...) -> Command<Message>` concrete: Rust performs no
implicit conversion on return, and generalizing the return type would obscure
the Command boundary without allowing heterogeneous branch types.

## Where Does Each Kind of State Belong?

The `Model` contains mutable behavioral state. A Component configuration may
contain immutable logical configuration and wiring, such as Ports and
SourceDescriptors. Component methods receive `&self`; after registration, the
configuration is observationally immutable. Behaviorally relevant mutation
belongs in the Model rather than behind interior mutability in the
configuration.

Layer state may contain deterministic mechanism state, such as a framing
buffer, but no ambient I/O resource. Runtime, Driver, and Source state may
contain operational resources such as sockets, tasks, clocks, and transport
correlation tables.

This separation keeps transition behavior reproducible without pretending that
live resources are pure values.

## Why Does `update` Receive No Runtime Context?

A transition should depend only on its current model and input message. Giving
it a runtime, clock, executor, or I/O capability would create an ambient path
around explicit commands and make controlled execution less trustworthy.

## Why Are Effect Descriptors Explicit Typed Values?

The runtime must be able to identify, interpret, trace, and replace every world
interaction. An `EffectDescriptor` is an inert typed description that can be
realized by a live Driver or answered by controlled behavior; an opaque async
closure cannot provide the same contract.

Pure synchronous closures or function pointers may still map an EffectOutcome
to a Component Message. They transform data and do not perform the effect
themselves.

## When Should an Effect Discard Its Outcome?

Use `Command::effect_discarding_outcome` when the Component deliberately has no
behavioral reaction to success, typed failure, or effect-contract cancellation.
Best-effort printing is the motivating case: an artificial "print finished"
Message would communicate no application intent.

This mode should not be called *fire and forget*. Samara still owns the effect,
controlled execution still exposes and traces it, Drain still waits for it,
Cancel still aborts it, and Driver or runtime faults still surface. Only the
application continuation is absent. If any terminal outcome should affect the
Model or cause another Command, use `Command::effect` when the Message has the
canonical `From<EffectOutcome<...>>` conversion, or `Command::effect_with` for
an explicit mapper.

## Why Distinguish Error Data from Failure?

An Error is typed data that explains what went wrong. A failure is the semantic
occurrence in which an operation did not complete as intended and may carry
that Error. This produces natural Rust names such as `TcpError` and
`EffectOutcome::Failed(error)` without using *failure* for every terminal
condition. Normal Source ending and cancellation are terminal, but neither is
automatically a failure.

## Why Are Commands and Subscriptions Different?

A Command describes finite work requested by one transition. An
EffectDescriptor inside that Command normally produces one terminal
EffectOutcome; every outcome accepted by a running scope either maps exactly
once or closes an explicitly discarded-outcome obligation without a Message.
Whole-scope live abort may instead cancel the Driver without manufacturing an
outcome for an application that is ending. A Subscription declaratively
describes an ongoing Source the current Model wants maintained: it combines
stable identity, a comparable SourceDescriptor, and a reusable message mapper
for SourceEvents.

Keeping them distinct makes lifecycle, cancellation, and reconciliation
explicit instead of disguising long-lived work as a one-shot effect. The
surface symmetry stops where the semantics stop: an accepted EffectOutcome
occurs once; a Source may emit zero or more SourceEvents, and canceling a
Subscription does not inherently manufacture one final event.

## Why Distinguish SourceDescriptor, Subscription, and Source?

They answer three different questions:

- A SourceDescriptor says what ongoing production is desired and is comparable
  for reconciliation.
- A Subscription says that this Component wants that descriptor maintained
  under a stable identity and maps its events into Component Messages.
- A Source is the runtime-scoped realization that actually produces events.

Putting stable identity in the Subscription allows the same descriptor type to
serve multiple logical roles. Reserving Source for the running realization also
keeps sockets, tasks, and buffers out of inert API values.

When equal identity and SourceDescriptor retain the Source, reconciliation
atomically installs the latest mapper returned by `subscriptions()`. Messages
already created keep their meaning; later events use the latest projection.
This follows the declarative model: each projection reasserts the Component's
complete current desire without forcing an unchanged world resource to
restart.

Changing the descriptor is different: replacement is a hard private-generation
cutover. Undelivered old work is stale and never crosses through the new mapper.
Applications that need overlap declare two Subscription identities or carry a
domain generation explicitly.

## How Should Concrete Descriptor Types Be Named?

The descriptor traits already state the architectural role. Concrete types
should therefore name the declared intent directly—`StoreFrame`, `TcpBytes`,
or `Framed<S, D>`—rather than mechanically appending `EffectDescriptor` or
`SourceDescriptor`. A `Descriptor` suffix remains available when the semantic
name would otherwise be ambiguous.

Concrete inert types should not use `Source`, which is reserved for the running
realization. `StreamDescriptor<T>` uses the otherwise optional `Descriptor`
suffix because plain `Stream` could be mistaken for that running realization.
It names the logical stream independently of how the world realizes it;
adapter-specific assembly methods such as `bind_mpsc` name the Tokio mechanism.
A future bridge module may add neighboring bindings without changing the
descriptor's application-facing meaning.

## Why Have Both Layers and Drivers?

*Adapter* is the conceptual umbrella for boundary translation. The code-level
roles are more precise:

- A Layer is a compositional adapter. It transforms descriptors or event
  vocabularies inside the declarative boundary and behaves the same in live and
  controlled execution. It may hold deterministic runtime-scoped mechanism
  state, such as a framing buffer, but performs no ambient I/O.
- A Driver is a terminal adapter. A live execution profile supplies an
  `EffectDriver<D>` or `SourceDriver<D>` that realizes terminal descriptor `D`
  against Tokio and the surrounding world.

The descriptor presented by application code may therefore be composed rather
than terminal. For example, `Framed<TcpBytes, D>` is a composed
SourceDescriptor, while `TcpBytes` is the terminal SourceDescriptor that reaches
its SourceDriver. An application may give the composed descriptor a domain
alias, but that alias does not introduce another Samara lifecycle concept.

During reconciliation, Samara automatically lowers the composed descriptor to
a runtime-owned SourcePlan containing its ordered Layers, terminal descriptor,
and Subscription mapper. Applications bind live or controlled behavior only
for `TcpBytes` in this example; requiring them to register `Framed` or its Layer
again would duplicate intent already expressed in the descriptor.

Controlled execution supplies deterministic behavior for the same descriptor
contracts and must never silently fall back to a live Driver. Drivers are
selected during assembly rather than associated with a Component
implementation. Samara does not yet promise one universal `Layer` trait; the
category is durable even if different kinds of composition need different Rust
abstractions.

## Why Can't a Correlated Request Feel Exactly Like a Function Call?

An ordinary function call has an implicit continuation. Its physical call site
and dynamic stack frame remember where a particular result must return. A
request whose dispatch and reply are separated in time and topology has no
shared stack, so that information must become explicit somewhere.

Samara's ergonomic ceiling is therefore not to make a correlated request
indistinguishable from a normal function call. It is to have the user state the
continuation and any domain context exactly once while Samara owns transport
correlation and lifecycle bookkeeping. An API that claims to eliminate this
irreducible information is likely hiding magic or discarding semantics.

## Why Are Notification and Request APIs Symmetric?

The two Port interaction forms should teach each other. `Command::notify` accepts a
value implementing `Notification<P>` and requests one-way delivery through a
`Port<P>`. `Command::request` accepts a value implementing `Request<P>`, whose
associated Reply type defines the successful result, and uses the requester's
canonical `From<RequestOutcome<...>>` conversion. `Command::request_with`
accepts an explicit continuation instead.

This naming states the relevant intent in the same vocabulary at both layers:
notification values are notified, and request values are requested. An
explicit continuation on `Command::request_with` is meaningful request/reply
information, not incidental transport ceremony.

## Which Parts of Request Correlation Belong to Whom?

The request continuation is a one-shot message mapper: it says what Component
Message should be produced when the Request terminates. Each dynamic occurrence
is a `RequestInvocation` containing the Request value and a `ReplyTo` for the
provider. Opaque runtime bookkeeping distinguishes that invocation from every
other invocation. Captured values, such as a domain request ID, preserve the
application's reason for making the Request.

The runtime owns transport correlation, delivery, timeout, cancellation, and
late-reply bookkeeping. Components should never allocate or compare transport
correlation IDs. Domain correlation remains explicit application data because
only the application knows what a reply means.

## Why Is `ReplyTo` a Checked Obligation Rather Than Merely a Token?

A non-cloneable `ReplyTo` consumed by `Command::reply` prevents a provider from
replying twice through the same authority. Rust ownership alone cannot require
the provider to use that authority: it can still be dropped, stored
indefinitely, or deliberately forgotten. The type therefore provides an
at-most-once guarantee, not an exactly-once guarantee.

Samara should make an unresolved reply difficult to miss through layered
checks:

- `ReplyTo` is marked `#[must_use]` so discarding a `ReplyTo`-valued expression
  receives a compiler diagnostic. This lint is an early warning, not proof that
  a bound value was eventually consumed.
- When reply-obligation diagnostics are enabled, especially in controlled
  conformance tests, dropping an armed reply obligation records a violation.
  `Drop` must not panic or alter application semantics.
- The obligation transfers into `Command::reply`, rather than disappearing when
  that command is constructed, so constructing and then discarding a reply
  command remains detectable.
- Runtime-owned request bookkeeping is authoritative. A later lifecycle
  contract may turn an unresolved obligation into an explicit terminal outcome
  such as `RequestError::ReplyAbandoned`; correctness must never depend on a
  destructor running.

Phase 5 implements only successful `RequestOutcome::Replied`. An unanswered
Request remains runtime-owned and pending until controlled cancellation; it
does not yet manufacture failure, timeout, cancellation, or abandonment
outcomes. The exact lifecycle boundary remains an API decision. A normal
request might require a reply from the provider's handling transition, while a
future explicit delegation mechanism might transfer the obligation and relax
ordering guarantees. That choice changes observable request semantics and
should be made explicitly.

## Why Do Request Outcomes Return as Messages Rather Than Futures?

Components do not suspend inside `update` and do not retain runtime-owned
futures. A Request is declared through `Command::request` now; its typed
`RequestOutcome` arrives later through the Component's ordinary Message and
transition path. This keeps request/reply compatible with pure transitions,
controlled execution, and per-Component serialization. `RequestError` carries
runtime-visible failures other than timeout and cancellation; the exact outcome
and error variants remain subject to the lifecycle policy.

## Why Are Ports the Normal Component Dependency Boundary?

A Port exposes a provider-neutral typed protocol rather than another
Component's private message enum. Program assembly binds that protocol to a
concrete provider, allowing consumers to remain unchanged when the provider is
replaced, wrapped, or controlled in a test.

The provider's private Component Message implements
`From<Protocol::Message>`. Consequently, `bind_port` states only which exact
named Port reaches which provider; it does not repeat an otherwise canonical
conversion function at every binding site. This also makes acceptance of a
Protocol visible on the provider Message type itself.

Ports are named rather than globally selected by protocol type. Two dependencies
may implement the same protocol while representing distinct roles, such as a
primary and fallback service.

## Why Is Program Assembly Validation Deliberately Narrow?

`ProgramBuilder::build()` can reject facts that assembly directly owns:
duplicate Component identities, duplicate `(Protocol type, PortId)`
declarations, Ports not bound exactly once, and providers registered in a
different builder. Failing there gives both profiles the same valid logical
Program before runtime startup.

It cannot inspect arbitrary fields inside Component configuration or predict
behavior-dependent `Command::send` edges. Port cycles are therefore legal and
not detected, and Samara does not pretend to build a closed static dependency
graph. A missing direct-send target is diagnosed only if that Command is later
interpreted.

## When Is a Direct Component Reference Appropriate?

A typed Component reference is a useful lower-level option when tight coupling
to the target's complete message API is deliberate. Reusable application
boundaries should prefer Ports so private provider messages do not become a
transitive dependency of every consumer.

## Why Are Components Larger Than Ordinary Rust Types?

A Component boundary adds a model, messages, transitions, effects,
subscriptions, and explicit wiring. That cost is justified only when it buys
meaningful ownership, isolation, testability, or reuse across live and
controlled execution. Not every struct, function, or asynchronous operation
should become a Component.

## What Must Remain the Same Across Live and Controlled Execution?

The Component implementation and configuration, Model, Messages, Commands,
EffectDescriptors, SourceDescriptors, Subscriptions, Protocols, Layers, and
pure message-mapping logic are the same program in both profiles. Runtime
decisions and terminal world-facing bindings differ. Convenience APIs must
preserve that shared path rather than creating a second testing-only
application model.

## What Does Successful Live Ingress Mean?

Under ADR-0004, a successful `ComponentHandle::send` means the Message
has crossed the admission boundary and is owned for runtime-managed delivery.
It does not mean the Component transition has completed. A send racing shutdown
or a runtime fault is either accepted under the applicable closure policy or
rejected explicitly; there is no ambiguous successful-but-never-admitted
result.

`SourceSink` uses the same acceptance idea. Success means an event was accepted
from that active Source generation, and successful calls from one Source retain
their acceptance order. It still does not mean the mapped Component transition
has run.

## Why Does the Initial Live Runtime Use Unbounded Internal Delivery?

It is the smallest mechanism that makes successful admission honest without
prematurely choosing queue capacities, shedding, fairness, or async-pressure
APIs. While a healthy runtime scope is running, accepted work is not silently
dropped because an internal queue filled.

This simplicity has a real cost: sustained overload may grow memory without
bound, and memory exhaustion is not a supported recovery mode. The first-party
`mpsc` bridge retains only the pressure supplied by the application's upstream
Tokio channel until Samara receives an item. Phase 6 load evidence is
characterization, not a capacity or throughput guarantee. A future bounded
policy is change-controlled because it changes what acceptance means.

## What Is the Difference Between Drain and Cancel?

`Shutdown::Drain` atomically closes external ingress, disables new or
restarted Sources, stops current Sources, then recursively processes Messages
and Source deliveries accepted before that cutoff plus finite work causally
emitted while draining. Timers, effects, and Requests remain eligible, so Drain
may wait forever for a hung Driver, unanswered Request, distant or recurring
timer, or self-sustaining application.

`Shutdown::Cancel` is a scope abort. It closes ingress, stops application
driving, cancels queued and deferred obligations and Driver tasks, and closes
all runtime ownership. Because the application is ending, it does not fabricate
EffectOutcomes, SourceEvents, RequestOutcomes, or Messages solely to announce
that abort. This differs from an explicit effect-contract cancellation outcome,
which is typed data and completes the effect exactly once when accepted: it
invokes the canonical or explicit mapper for `Command::effect` or
`Command::effect_with`, or schedules no Message for
`Command::effect_discarding_outcome`.

Successful shutdown always means no runtime-owned task or semantic obligation
remains. `completed` and `cancelled` describe semantic obligations rather than
task mechanics, but their exact diagnostic counts are deliberately not a v0
conformance promise.

## How Does a Live Source End?

An active Source generation accepts at most one terminal condition. The first
accepted `SourceSink::end`, accepted `SourceSink::fail`, or SourceDriver return
wins and produces one terminal event. If cancellation, replacement, shutdown,
or runtime fault withdraws the generation first, it produces no terminal event
and later sink calls return `DriverStopped`.

A SourceDriver may therefore return normally without spelling `sink.end()`;
Samara treats active silent return as `Ended`. Successful calls from one Source
preserve acceptance order, so chunks cannot be overtaken by EOF finalization.
An ended Source remains inactive even if the same Subscription desire remains;
there is no hidden automatic restart.

## Why Must a Decoder Finalize Explicitly?

One raw chunk is not the same thing as the end of a byte stream. ADR-0004
therefore requires a pure Decoder EOF operation. On normal underlying
`Ended`, the Framed Layer invokes it exactly once, emits any final frames in
order, then emits `Ended`. A finalization Error emits one decode failure and no
`Ended`.

Underlying Source failure, an earlier decode failure, replacement,
cancellation, shutdown, and runtime fault are not normal EOF and do not invoke
finalization. This prevents the Layer from silently discarding partial state or
pretending a transport failure was a complete stream.

## Why Are the Initial `mpsc` and TCP Bridges Narrow?

A Tokio `mpsc::Receiver` is a unique, single-consumer resource. The accepted
first-party binding consumes it on first activation; a competing claimant or
later activation after end or cancellation faults explicitly. Fabricating
another `Ended` would hide that no new receiver exists, and
`StreamDescriptor<T>` deliberately has `Infallible` Source Error data.

The accepted TCP Driver likewise owns one connection per Source realization,
emits `bytes::Bytes`, maps connection/read errors and peer EOF, and closes on
cancellation. It does not retry, reconnect, frame, or interpret domain data.
Those choices belong in Layers or Component logic where they remain visible
and controllable.

## What Does This Document Deliberately Not Settle?

The Phase 2 API contract freezes the Component-kernel signatures and records
later slices separately. ADR-0004 selects only the simplest v0 live
mechanisms. Bounded pressure and overload controls, shutdown deadlines and
escalation, exact shutdown diagnostic counts, per-effect cancellation,
Driver recovery, restartable or shared bridges, the broader first-party Tokio
module organization, the Rust shape of general Layer/profile bindings, Request
lifecycle policy, notification delivery failures, public live and
domain-payload trace APIs, and runtime topology remain separate decisions.
ADR-0005 additionally freezes only its narrow raw, pooled, no-redirect,
no-retry HTTP Effect; it does not settle higher-level endpoint or client policy.
Those choices should follow the guidance above rather than being inferred from
the first implementation.
