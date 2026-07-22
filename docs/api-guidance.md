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
explanatory path from input, through state transition and effect intent, back to
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

The `Model` contains mutable behavioral state. The Component value may contain
immutable logical configuration and wiring, such as Ports and source
descriptors. Runtime and adapter state contains operational resources, such as
sockets, tasks, partial buffers, clocks, and transport correlation tables.

This separation keeps transition behavior reproducible without pretending that
live resources are pure values.

## Why Does `update` Receive No Runtime Context?

A transition should depend only on its current model and input message. Giving
it a runtime, clock, executor, or I/O capability would create an ambient path
around explicit commands and make controlled execution less trustworthy.

## Why Are Effects Explicit Typed Values?

The runtime must be able to identify, interpret, trace, and replace every world
interaction. A typed intent can be executed by a live adapter or intercepted by
a controlled world; an opaque async closure cannot provide the same contract.

Pure synchronous closures or function pointers may still map an effect outcome
to a message. They transform data and do not perform the effect themselves.

## Why Are Commands and Subscriptions Different?

A command describes finite work requested by one transition. A subscription
declaratively describes an ongoing source the current model wants maintained.
Keeping them distinct makes lifecycle, cancellation, and reconciliation
explicit instead of disguising long-lived tasks as one-shot effects.

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

The two Port interaction forms should teach each other. `Cmd::notify` accepts a
value implementing `Notification<P>` and requests one-way delivery through a
`Port<P>`. `Cmd::request` accepts a value implementing `Request<P>`, whose
associated reply type defines the successful result, plus a result mapper that
turns the eventual `RequestOutcome` into the requester's Msg.

This naming states the relevant intent in the same vocabulary at both layers:
notification values are notified, and request values are requested. The extra
continuation on `Cmd::request` is meaningful request/reply information, not
incidental transport ceremony.

## Which Parts of Request Correlation Belong to Whom?

The result mapper represents the continuation: what message should be produced
when the request terminates. An opaque runtime token distinguishes one dynamic
invocation from every other invocation. Captured values, such as a domain
request ID, preserve the application's reason for making that request.

The runtime owns transport correlation, delivery, timeout, cancellation, and
late-reply bookkeeping. Components should never allocate or compare transport
correlation IDs. Domain correlation remains explicit application data because
only the application knows what a reply means.

## Why Is `ReplyTo` a Checked Obligation Rather Than Merely a Token?

A non-cloneable `ReplyTo` consumed by `Cmd::reply` prevents a provider from
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
- The obligation transfers into `Cmd::reply`, rather than disappearing when
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
futures. A request is declared through `Cmd::request` now; its typed
`RequestOutcome` arrives later through the Component's ordinary message and
transition path. This keeps request/reply compatible with pure transitions,
controlled execution, and per-Component serialization. `RequestError` carries
runtime-visible failures other than timeout and cancellation; the exact outcome
and error variants remain subject to the lifecycle policy.

## Why Are Ports the Normal Component Dependency Boundary?

A Port exposes a provider-neutral typed protocol rather than another
Component's private message enum. Program assembly binds that protocol to a
concrete provider, allowing consumers to remain unchanged when the provider is
replaced, wrapped, or controlled in a test.

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

The Component, model, messages, commands, subscriptions, protocols, and pure
mapping logic are the same program in both profiles. Only runtime decisions and
world-facing bindings differ. Convenience APIs must preserve that shared path
rather than creating a second testing-only application model.

## What Does This Document Deliberately Not Settle?

Exact type names and signatures remain open while the API sketch is being
workshopped. Request deadline policy, cancellation taxonomy, notification
delivery failures, shutdown policy, and runtime topology also require separate
decisions. Those choices should follow the guidance above, but this document
does not make them implicitly.
