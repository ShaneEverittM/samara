# ADR 0006: Live Port Ingress

- Status: Accepted
- Date: July 25, 2026
- Decision owners: Samara maintainers
- Extends: [ADR 0004](0004-initial-live-runtime-semantics.md)

## Context

ADR-0004 introduced `ComponentHandle<C>` as a live capability for surrounding
Tokio code to submit a Component's private Message type. That is useful for
tightly coupled host integration, but it is the wrong boundary when the host
should depend on a provider-neutral Protocol. Components can already issue
Notifications and correlated Requests through an inert `Port<P>`; live host
code should be able to use that same declared Port binding without learning the
provider Component's private Message vocabulary.

An external Request differs from a Component-issued Request at its completion
boundary. A Component cannot await and must receive a `RequestOutcome` through
its Message path. Surrounding Tokio code can await, and the useful successful
result is the Request's associated `R::Reply` itself. The API must add that
convenience without placing a future or runtime capability inside a Component,
bypassing runtime-owned correlation, or silently choosing the still-deferred
application-level Request timeout and abandonment policies.

The new surface also has to preserve ADR-0004's atomic admission and structured
shutdown rules. In particular, cancelling a host future must not create an
implicit per-Request cancellation protocol or let accepted work disappear from
Drain accounting.

## Decision

### `Port` remains inert; `PortHandle` is the live capability

`LiveRuntime::port_handle(&Port<P>)` creates a cloneable `PortHandle<P>`. Handle
creation validates that the supplied Port belongs to the exact Program owned by
that LiveRuntime and that the built Program contains its exact Protocol-and-name
binding. A foreign Port or a same-named logical lookalike from another Program is
rejected synchronously with `RuntimeError`; `ProgramBuilder::build` continues to
reject unbound declarations, and `Port<P>` keeps Protocol identity typed.

The handle retains logical Port identity and the runtime's live admission
capability. It does not expose or retain public access to the bound provider,
its Model, a Component transition, a Tokio channel, or runtime topology.
`Port<P>` remains the inert value stored in Component configuration and used by
`Command::notify` and `Command::request`; it does not gain live methods.

The accepted public shape is:

```rust,ignore
impl LiveRuntime {
    pub fn port_handle<P: Protocol>(
        &self,
        port: &Port<P>,
    ) -> Result<PortHandle<P>, RuntimeError>;
}

impl<P: Protocol> PortHandle<P> {
    pub async fn notify<N: Notification<P>>(
        &self,
        notification: N,
    ) -> Result<(), RuntimeError>;

    pub async fn request<R: Request<P>>(
        &self,
        request: R,
    ) -> Result<R::Reply, RuntimeError>;
}
```

Applications obtain Component and Port handles from the assembled LiveRuntime
before `LiveRuntime::spawn` consumes it, then retain those handles beside the
owning `RuntimeTask`. `ComponentHandle` remains the deliberate private-Message
ingress; `PortHandle` is the provider-neutral Protocol ingress.

### Port ingress shares the live admission cutoff

`ComponentHandle`, `PortHandle`, and the owning `RuntimeTask` share one live
scope and one atomic external-ingress cutoff. Calling an async PortHandle method
does no work until its future is polled. Its first active poll attempts
admission. If admission is already closed, the operation resolves with
`RuntimeError` and no Protocol Message reaches the provider.

A successful `PortHandle::notify` means the Notification was accepted for
runtime-managed conversion and delivery. It does not mean that the provider's
transition completed. Once a `PortHandle::request` has passed admission and is
waiting for its Reply, the Request is likewise runtime-owned. The method has no
separate public "accepted" result because its successful return is reserved for
the eventual `R::Reply`.

An operation racing Drain, Cancel, or runtime fault is either admitted before
the shared cutoff and governed by the corresponding ownership rules, or
rejected with `RuntimeError`. There is no successful-but-unadmitted state.
Healthy post-admission delivery uses ADR-0004's unbounded internal mechanism;
this ADR adds no capacity, fairness, or backpressure guarantee.

Runtime interpretation uses the same Port binding, `Protocol::Message`
conversion, provider `From<Protocol::Message>` conversion, per-Component
serialization, and causal delivery path as `Command::notify` and
`Command::request`. Host ingress does not directly invoke the provider.

### A host Request awaits its typed Reply directly

For each admitted `PortHandle::request`, the runtime creates the same opaque,
one-shot transport correlation and `ReplyTo<R::Reply>` authority used for a
Component-issued Request. The provider receives an ordinary
`RequestInvocation<P, R>` and replies only by returning `Command::reply` from a
transition.

When that Reply wins the Request's terminal race, the host future resolves to
`Ok(R::Reply)`. The runtime does not create a requester Component, a Component
Message mapper, or `RequestOutcome::Replied` for this live host boundary. Domain
negative replies remain ordinary values in `R::Reply`, exactly as they do for a
Component-issued Request.

This direct result does not weaken the Message-only state rule. Host code is
outside the Samara Program boundary and cannot mutate Component state through
the handle. If a Reply should affect Samara application state, it must re-enter
through an admitted Component Message or Protocol operation.

The runtime owns transport correlation and counts the admitted operation as one
outstanding Request semantic obligation. The host waiter is not an additional
semantic work unit. Concurrent requests remain distinct even when their typed
values are equal. This ADR promises each request-to-reply causal edge but no
relative live order among independent PortHandle calls or Replies.

### Drain retains admitted Port work

`Shutdown::Drain` closes ComponentHandle and PortHandle admission at the same
cutoff. Notifications admitted before that cutoff remain eligible for provider
delivery. Requests admitted before the cutoff remain outstanding finite work;
their provider Messages, Replies, and causally emitted finite work remain
eligible during Drain.

An unanswered host Request can therefore keep both its request future and
`RuntimeTask::shutdown(Shutdown::Drain)` pending forever. Drain supplies no
implicit Request timeout, abandonment detection, or escalation. The host may
select Cancel or apply its own operational deadline around shutdown, but timing
out the host's wait does not itself cancel the admitted Samara Request.

A correctly completed Drain cannot discard an outstanding host Request and then
report clean closure. It either observes the Reply and discharges the obligation
or remains pending.

### Cancel and closure wake host waiters without an application outcome

`Shutdown::Cancel` closes Port admission, stops application driving, and removes
admitted but incomplete host Requests as part of the ordinary whole-scope abort.
Every affected `PortHandle::request` waiter is woken and resolves to
`Err(RuntimeError)`. A non-fault clean scope closure or runtime-owner termination
that wins before the Reply uses the same host-boundary error channel.

This error is not `RequestOutcome::Cancelled`, `RequestError`, or a fabricated
Component Message. ADR-0004's rule that whole-scope termination manufactures no
application outcome remains intact. This ADR settles only how an external Tokio
waiter learns that its live runtime can no longer produce the Reply; it does not
activate the deferred in-band Request cancellation variants.

Reply completion and scope closure are terminal alternatives. Whichever is
accepted first resolves the host operation, and the other path cannot resolve it
again.

### Runtime faults preserve their diagnostic identity

If a live runtime fault wins before the Reply, every affected host Request
resolves to `Err` containing that scope's preserved `RuntimeError`, rather than a
generic clean-closure replacement. Later `PortHandle::notify` and
`PortHandle::request` calls are rejected with the same fault, matching the
existing ComponentHandle fault boundary.

Protocol construction, binding conversion, correlation, and provider delivery
remain runtime-interpreted application/mechanism boundaries. A panic or type
violation there follows the existing live runtime-fault policy; PortHandle must
not move provider conversion into a direct host-to-Component call or leak a
panic-based expected-error channel.

### Dropping a waiter does not cancel admitted work

Dropping an unpolled `PortHandle::request` future admits nothing. After
admission, dropping or timing out the host future relinquishes only that host
task's interest in observing the result. It does not retract the provider
Message, consume or invalidate `ReplyTo`, remove the outstanding Request, or
relax Drain.

The runtime continues to own the Request until a Reply, Drain's continued wait,
whole-scope Cancel, or runtime fault discharges it. If the provider replies after
the waiter was dropped, the runtime consumes the correlation and completes the
obligation without creating another destination for the value.

This rule deliberately avoids inventing per-Request cancellation and late-Reply
semantics. A future explicit cancellation API would require a separate contract
covering provider visibility, terminal arbitration, and late `ReplyTo` use.

### The handle is live-only

`PortHandle` is a capability of one assembled LiveRuntime and must not be stored
in a Component, supplied to `update`, or exposed by ControlledRuntime. Controlled
tests continue to drive deterministic Component inputs and inspect Protocol work
through the existing controlled harness.

A future controlled host-Port API would need deterministic scheduling and a
synchronous or explicitly driven Reply observation model. This ADR neither
requires nor reserves that shape. The Program, Protocol, Port binding, provider,
RequestInvocation, and Reply semantics used after ingress remain common across
profiles; only the surrounding host capability is live-specific.

## Consequences

### Positive

- Tokio hosts can use provider-neutral Protocols instead of a provider's private
  Component Message enum.
- A successful host Request has the natural `R::Reply` result while Components
  retain Message-based continuations and pure transitions.
- Port ingress shares one honest admission, Drain, Cancel, and fault boundary
  with existing live ingress.
- Runtime-owned correlation and `ReplyTo` remain the sole provider reply path.
- Dropped host futures cannot silently change application or shutdown semantics.

### Negative

- A host Request and Drain can both wait forever for a provider that never
  replies.
- Timing out or dropping the host wait does not cancel resource use inside the
  Samara runtime.
- Clean whole-scope cancellation is reported to the host as `RuntimeError`, even
  though it is not an application or Driver fault.
- The live-only convenience has no identical ControlledRuntime handle API.
- Unbounded admitted Port traffic has the same memory-exhaustion limitation as
  existing live ingress.

## Failure Modes

- Handle construction with a foreign Port or one absent from the runtime's exact
  built bindings fails before spawn with `RuntimeError`.
- Notify or Request admission after Drain, Cancel, clean closure, or fault fails
  with `RuntimeError` and performs no provider delivery.
- An admitted Notification can still be discarded only at ADR-0004's explicit
  Cancel or fault cutover; this ADR adds no provider-completion acknowledgement.
- An admitted Request with no Reply remains pending and can prevent Drain from
  completing.
- Cancel or non-fault scope closure before Reply wakes the host waiter with a
  closure `RuntimeError` and creates no RequestOutcome.
- Runtime fault before Reply wakes the waiter with the preserved fault.
- Dropping the host waiter after admission leaves the Request owned and can
  therefore leave Drain pending.
- Provider or conversion panics remain runtime faults, not typed domain Replies.

## Required Evidence

- `LiveRuntime::port_handle` accepts an exact Port from its Program and rejects a
  foreign or same-named lookalike Port from another Program. Existing Program
  build tests continue to reject unbound Port declarations.
- A cloned PortHandle admits Notifications through the selected binding, and the
  provider receives its ordinary private Component Message exactly once without
  overlapping another transition.
- Notify after shutdown or fault is rejected, and a notify racing the cutoff is
  either accepted and owned or rejected without delivery.
- A host Request reaches the provider as a typed `RequestInvocation`, an ordinary
  `Command::reply` resolves it to the exact `R::Reply`, and no RequestOutcome or
  requester Component Message is created.
- Concurrent equal-typed host Requests remain correctly correlated when their
  Replies complete in a different order.
- Drain closes new Port admission, delivers admitted Notifications, retains an
  admitted Request, and completes only after its Reply and causal work complete.
- Cancel and clean owner closure wake every admitted host Request with
  `RuntimeError`, leave zero owned work after successful shutdown, and invoke no
  application Request mapper.
- A runtime fault wakes every admitted host Request with the same preserved
  `RuntimeError` surfaced by the RuntimeTask and rejects later Port ingress with
  that fault.
- Dropping an unpolled request future admits nothing; dropping a pending admitted
  waiter does not cancel provider delivery or release Drain early.
- Component and Port ingress share one atomic cutoff under a forced shutdown
  race, without asserting queue topology or a global order.
- Compile-contract evidence shows PortHandle exposes no Model access and cannot
  substitute for an inert Port in Command or ControlledRuntime APIs. Because
  Rust cannot prohibit arbitrary `Send` Component fields, documentation and
  conformance review also enforce that live handles stay outside Components.

## Explicitly Unresolved

- per-Request timeout, deadline, cancellation, abandonment, delegation, retry,
  or late-Reply policy;
- provider-visible cancellation or a cancellable `ReplyTo`;
- Notification provider-completion or delivery-failure acknowledgement beyond
  admission and whole-scope cutovers;
- a controlled host-Port capability or deterministic host-wait abstraction;
- FIFO among independent handle clones, priorities, bounded capacity,
  backpressure, fairness, shedding, or admission quotas;
- richer clean-closure error taxonomy and exact public RuntimeError variants;
- host-signal integration, shutdown deadlines, or automatic Drain-to-Cancel
  escalation; and
- public live tracing or observation of host Port operations.

## Rollback

`PortHandle` and `LiveRuntime::port_handle` can be removed without changing the
inert Port, Protocol, provider, Command, or ControlledRuntime contracts. Hosts
can fall back to a narrow ingress Component and `ComponentHandle`.

Changing the shared admission cutoff, direct `R::Reply` result, Drain retention,
Cancel/fault wakeup, dropped-waiter non-cancellation, or live-only scope requires
a superseding ADR and matching executable evidence. Internal event, channel,
correlation-table, oneshot, task, and routing representations may change without
an ADR when all behavior above remains unchanged.
