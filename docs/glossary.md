# Samara Glossary

- Status: Draft
- Date: July 21, 2026
- Scope: Canonical project vocabulary and important distinctions

This document defines how Samara currently uses its growing vocabulary. It is a
reference for API design, implementation, testing, and documentation—not a
substitute for the behavioral contracts in the vision and ADRs.

The concepts below are canonical unless marked historical. Exact Rust type and
method names remain provisional until the first public API is frozen.

## Program and State

**Samara program** — A declared collection of Components and their protocol,
effect, and subscription contracts. It contains logical application structure,
not live Tokio resources.

**Program assembly** — The construction of a Samara program, including
Component registration, named Port declaration, and provider binding.

**Program boundary** — The scope within which Samara's guarantees apply. Code
outside this boundary is part of the surrounding world and interacts with the
program through explicit adapters or ingress APIs.

**Surrounding world** — Everything a Samara program does not own, including
external systems, clocks, I/O, and inputs. Live execution interacts with the
real surrounding world; controlled execution substitutes a controlled world.

**Component** — Samara's topology-neutral unit of state and behavior. A
Component has stable logical identity, owns one Model, accepts typed messages,
applies one transition at a time, emits commands, and declares subscriptions.
The name remains provisional, but it is the current canonical replacement for
the historical term *Actor*.

**Component value** — The immutable logical configuration and wiring used by a
Component implementation, such as Ports and source descriptors. It is distinct
from the mutable Model and from runtime-owned operational resources.

**Model** — The mutable behavioral state exclusively owned by one Component.
Sockets, tasks, clocks, transport correlation tables, and partial I/O buffers
are operational state and do not belong in the Model unless their logical
meaning is represented as ordinary data.

**Msg / message** — The only input that may trigger a Component transition.
User intent, domain events, effect outcomes, source events, request outcomes, and
behaviorally relevant failures all enter application logic as messages.

**Transition / `update`** — One deterministic application of a Msg to a Model,
producing committed state and explicit commands. Transitions for one Component
never overlap.

**Practical purity / observational purity** — The requirement that a
transition's observable result depend only on its Model and Msg. Exclusively
owned in-place mutation is permitted when it is observationally equivalent to
producing a new Model value.

**Runtime** — The machinery that owns Models, delivers messages, invokes
transitions, interprets commands, reconciles subscriptions, supervises work,
and enforces Samara's execution semantics.

**Runtime topology** — The runtime's internal arrangement of tasks, mailboxes,
threads, queues, or loops. Topology is not a public semantic contract unless a
specific observable property is deliberately promised.

## Intent, Effects, and Ongoing Work

**Cmd / command** — An inert value describing finite work requested by a
transition. A command may wrap an Effect, schedule a message, communicate with
another Component, issue a correlated request, or emit a reply; *command* and
*effect* are therefore not synonyms.

**Effect / effect intent** — A typed, interceptable description of one finite
world-facing interaction. A live adapter executes it; controlled execution
supplies its outcome without invoking the live world.

**Result mapper** — Pure synchronous application logic that converts an effect,
source, or request outcome into a Msg. For a correlated request it is also the
continuation. Mapper object identity is not itself part of command semantics.

**Source / source descriptor** — Comparable logical configuration for an
ongoing event producer. Operational resources such as sockets, tasks, and
partial buffers are created and owned behind this descriptor.

**Subscription** — A model-derived declaration that the runtime should maintain
an ongoing Source under a stable Component-local identity. Declaring a
Subscription does not itself start work.

**Desired subscription** — A Subscription returned from the Component's current
Model during reconciliation.

**Active subscription** — The runtime-owned work currently maintained for a
desired Subscription.

**Subscription reconciliation** — The post-transition comparison of desired
and active subscriptions that starts, retains, replaces, or cancels active
source instances.

**Adapter** — Code that interprets or composes a declared Effect or Source
boundary. A live adapter may interact with Tokio and the surrounding world; a
controlled interpreter provides deterministic behavior for the same logical
contract.

**Mechanism** — A reusable execution capability owned by the runtime or a
low-level adapter, such as scheduling, transport I/O, cancellation, or task
supervision.

**Policy** — An application or protocol decision such as retry, reconnect,
framing, timeout meaning, or domain error mapping. Policy must not be hidden in
runtime-owned mechanism.

**Structured concurrency / runtime-owned scope** — The rule that every
Samara-authorized asynchronous activity remains owned, cancellable, and
accountable through a runtime scope. Detached work is non-conforming.

## Component Interaction

**Protocol** — A provider-neutral typed vocabulary exposed through a Port rather
than through a provider Component's private Msg type. Qualify network formats as
*wire protocols* to avoid ambiguity.

**Port** — A named, inert logical dependency on a Protocol. A Port contains no
provider reference, channel, runtime handle, or lookup capability.

**Provider** — The concrete Component selected during program assembly to
receive values sent through a Port.

**Requester** — A Component that issues a Request through a Port. It owns the
continuation and any domain correlation while the runtime owns transport
correlation.

**Port binding** — The assembly-time mapping from one exact named Port to a
provider and its private message vocabulary. Multiple named Ports of the same
Protocol may be bound independently.

**Notification** — A one-way protocol value implementing `Notification<P>` and
sent through `Cmd::notify`. It has no correlated terminal result for the sender.
Notification delivery-failure policy remains unresolved.

**Request** — A protocol value implementing `Request<P>` with a statically
associated `Reply` type. `Cmd::request` issues it as a finite correlated
request/reply command. Its eventual typed terminal outcome is transformed by a
result mapper into an ordinary requester Msg; the requester does not await
inside `update`.

**Reply** — The successful response type statically associated with a Request.
A provider emits a value of that type using the request's `ReplyTo` authority.

**Request outcome / `RequestOutcome`** — The requester-visible terminal outcome
of a Request, provisionally a reply, a `RequestError` failure, timeout, or
cancellation. The exact failure and deadline policy is not yet settled.

**`RequestError`** — The current provisional type for a Request's runtime-level
terminal failure. Domain-level negative replies remain ordinary Reply values.

**Continuation** — The request result mapper, including captured domain context,
that states what a separated outcome means to the requester and what Msg it
should produce.

**Transport correlation** — Opaque runtime bookkeeping that distinguishes one
dynamic Request invocation from every other invocation. Components neither
create nor compare transport correlation identifiers.

**Domain correlation** — Application-owned identity that explains the business
meaning of a request or reply. It is commonly captured by the continuation and
returned in the resulting Msg.

**`ReplyTo`** — The current provisional name for an opaque, one-shot authority
that permits a provider to emit the correctly typed reply. It is inert data,
not a channel, future, or runtime handle.

**`ComponentRef`** — The current provisional name for an inert typed logical
address used when direct coupling to another Component's complete Msg API is
deliberate.

**`ComponentHandle`** — The current provisional name for a live external-ingress
capability available at the Samara program boundary. Unlike a `ComponentRef`, it
is a runtime capability and must not be available inside Components.

## Execution and Ordering

**Live execution** — Execution using real Tokio scheduling, time, I/O, and
external systems. It preserves Samara's Component and causal guarantees but
does not deterministically order independent events.

**Controlled execution** — Execution in a closed, runtime-owned world where
inputs, effects, time, randomness, identifiers, and scheduling decisions are
controlled or seeded reproducibly. It is the guarantee-bearing term; simulation
is one use case for it.

**Controlled world** — The scripted or seeded substitute for the surrounding
world used during controlled execution. Missing controlled behavior fails
explicitly rather than falling back to live behavior.

**Logical time / controlled time** — Runtime-controlled time that can advance
manually or automatically without wall-clock sleeping.

**Runtime semantics** — The observable scheduling, lifecycle, and tie-breaking
rules whose sameness is part of a controlled-determinism claim.

**Transition determinism** — Equivalent Model and Msg inputs produce equivalent
next state and commands for a conforming Component.

**Program-wide controlled determinism** — Identical program, controlled world,
inputs, seeds, and runtime semantics produce the same whole-program semantic
trace, final state, causal relationships, and scheduled future work.

**Per-Component serialization** — A Component's transitions never overlap and
form one local history. This does not imply FIFO delivery from every source or a
program-wide order.

**Causal edge / causal ordering** — A required predecessor relationship
established by the program, such as command-to-outcome, send-to-delivery, or
request-to-reply.

**Independent events** — Events with no causal or explicitly sequenced
relationship. Samara promises no relative live order between them.

**Explicit FIFO / explicit sequencing** — Ordering guaranteed by a particular
delivery or work contract rather than inferred from runtime topology.

**Global total order** — One comparable sequence containing every program
event. Samara explicitly does not promise this for independent live events.

**Quiescence** — The absence of immediately runnable work. Work may still be
waiting for logical time, a controlled input, or an external event.

## Observability and Correctness

**In-band semantic outcome** — A behaviorally relevant result delivered to a
Component as a Msg. Components can make decisions from it.

**Semantic trace** — An out-of-band structured record of transitions, intents,
lifecycle outcomes, time, and causation. Observing the trace cannot feed data
back into Components or otherwise alter semantic behavior.

**Runtime diagnostic** — Operational information such as task identity, thread
placement, queue depth, or incidental sequence number. It is not an application
semantic contract unless explicitly promoted into one.

**Conformance** — The combined runtime, Component-author, and adapter-author
obligations required for Samara's guarantees to hold.

## Distinctions Worth Preserving

| Do not collapse | Distinction |
| --- | --- |
| Command and Effect | A Command is the broader finite-work envelope; an Effect is one typed world-facing intent it may contain. |
| Source and Subscription | A Source describes an event producer; a Subscription adds Component-local desire, identity, and mapping. |
| Port and provider | A Port is an inert dependency; the provider is the Component selected during assembly. |
| Notification and Request | `Notification<P>` is one-way input issued by `Cmd::notify`; `Request<P>` has an associated Reply and is issued by `Cmd::request` with a continuation. |
| Protocol and wire protocol | A Samara Protocol is a typed Component boundary; a wire protocol defines external data exchange. |
| Msg and event | Boundary-specific events are mapped into Msg values; trace events remain out of band. |
| Per-Component serialization and ordering | Non-overlapping transitions do not create a global order or imply unspecified FIFO behavior. |
| Controlled execution and simulation | Controlled execution names the semantic profile; simulation is one workload that uses it. |
| Domain and transport correlation | Applications own meaning; the runtime owns delivery bookkeeping. |
| `ComponentRef` and `ComponentHandle` | A reference is inert logical wiring; a handle is a live ingress capability. |

## Historical or Avoided Vocabulary

| Historical or ambiguous term | Current vocabulary | Guidance |
| --- | --- | --- |
| Actor | Component | Use *Component* for Samara's topology-neutral application unit. Actor language survives in older PoC documents. |
| `ActorRef`, `Addr`, `PortRef`, `RuntimeRef` | `ComponentRef`, `ComponentHandle`, or `Port` | These older names combined distinct logical-address, live-capability, and dependency roles. Choose the current term that states the role. |
| `ask` | `request` | Use `Cmd::request` for command-now, message-later request/reply; *ask* can misleadingly suggest an awaitable future. |
| `tell` | `notify` or `send` | Use *notify* for provider-neutral one-way Protocol interaction and *send* for deliberate direct delivery to a Component Msg API. |
| `ReplyToken` | `ReplyTo` | `ReplyTo` is the current provisional spelling; do not canonize both. |
| Simulated execution | Controlled execution | Use *controlled* for the semantic profile; use *simulation* for the workload or product use case. |
| Mailbox loop | Runtime topology | A mailbox or event loop may be an implementation technique, not the definition of a Component. |
| Effect handler, interpreter, backend, adapter | Interpreter or adapter, qualified by role | Use *interpreter* generically for executing a contract and *adapter* for a concrete or compositional boundary; qualify live versus controlled behavior. |
