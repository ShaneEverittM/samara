# Samara API Guidance

- Status: Draft
- Date: July 21, 2026
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

## Why Distinguish Error Data from Failure?

An Error is typed data that explains what went wrong. A failure is the semantic
occurrence in which an operation did not complete as intended and may carry
that Error. This produces natural Rust names such as `TcpError` and
`EffectOutcome::Failed(error)` without using *failure* for every terminal
condition. Normal Source ending and cancellation are terminal, but neither is
automatically a failure.

## Why Are Commands and Subscriptions Different?

A Command describes finite work requested by one transition. An
EffectDescriptor inside that Command produces one terminal EffectOutcome. A
Subscription declaratively describes an ongoing Source the current Model wants
maintained: it combines stable identity, a comparable SourceDescriptor, and a
reusable message mapper for SourceEvents.

Keeping them distinct makes lifecycle, cancellation, and reconciliation
explicit instead of disguising long-lived work as a one-shot effect. The
surface symmetry stops where the semantics stop: an EffectOutcome occurs once;
a Source may emit zero or more SourceEvents, and canceling a Subscription does
not inherently manufacture one final event.

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

This does not yet settle whether a newly declared message mapper replaces the
prior mapper while equal identity and SourceDescriptor retain the Source. That
choice changes observable Messages and must be made explicitly before the API
is frozen.

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
associated Reply type defines the successful result, plus a request
continuation that turns the eventual `RequestOutcome` into the requester's
Component Message.

This naming states the relevant intent in the same vocabulary at both layers:
notification values are notified, and request values are requested. The extra
continuation on `Command::request` is meaningful request/reply information, not
incidental transport ceremony.

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
- Runtime-owned request bookkeeping is authoritative. At the request
  lifecycle boundary, an unresolved obligation becomes an explicit terminal
  outcome such as `RequestError::ReplyAbandoned`; correctness never depends on
  a destructor running.

The exact lifecycle boundary remains an API decision. A normal request might
require a reply from the provider's handling transition, while a future
explicit delegation mechanism might transfer the obligation and relax ordering
guarantees. That choice changes observable request semantics and should be made
explicitly; silently losing a `ReplyTo` is invalid under either policy.

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

## What Does This Document Deliberately Not Settle?

The Phase 2 API contract freezes the Component-kernel signatures and records
later compile-checked slices separately. The broader first-party Tokio bridge
module organization, the Rust shape of Layer and execution-profile bindings,
Request deadline and cancellation policy, notification delivery failures,
shutdown policy, semantic trace API, and runtime topology still require
separate decisions. Those choices should follow the guidance above, but this
document does not make them implicitly.
