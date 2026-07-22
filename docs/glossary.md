# Samara Glossary

- Status: Draft
- Date: July 21, 2026
- Scope: Canonical project vocabulary and important distinctions

This document defines how Samara currently uses its growing vocabulary. It is a
reference for API design, implementation, testing, and documentation—not a
substitute for the behavioral contracts in the vision and ADRs.

The concepts below are canonical unless marked historical. The Phase 2
candidate Rust surface follows these spellings. `api-contract.md` records which
API slice is frozen for each implementation phase and which policy-bearing
surfaces remain provisional.

## Program and State

**Samara program** — A declared collection of Components and their protocol,
effect, and subscription contracts. It contains logical application structure,
not live Tokio resources.

**Program assembly** — The construction of a Samara program, including
Component registration, named Port declaration, provider binding, and execution
profile selection.

**Program boundary** — The scope within which Samara's guarantees apply. Code
outside this boundary is part of the surrounding world and interacts with the
program through explicit adapters or ingress APIs.

**Surrounding world** — Everything a Samara program does not own, including
external systems, clocks, I/O, and inputs. Live execution interacts with the
real surrounding world; controlled execution substitutes a controlled world.

**Component** — Samara's topology-neutral unit of state and behavior. A
Component has stable logical identity, owns one Model, accepts typed Component
messages, applies one transition at a time, emits Commands, and declares
Subscriptions. The name is the canonical replacement for the historical term
*Actor*.

**Component implementation** — The Rust type and `impl Component` that define a
kind of Component. Its methods receive `&self`, signaling that the
implementation's configuration is observationally read-only during execution.
Because Rust permits interior mutability, preserving that property is also a
conformance obligation.

**Component configuration** — A particular immutable value of a Component
implementation type. It may hold logical wiring and declarative configuration,
such as Ports and SourceDescriptors. Behaviorally relevant mutable state does
not belong here.

**Model** — The mutable behavioral state exclusively owned by one Component.
Sockets, tasks, clocks, transport-correlation tables, and other operational
resources are not Model state. A partial buffer belongs in the Model only when
the application intentionally treats its contents as behavioral state rather
than as hidden Layer or Driver machinery.

**Component message / `Component::Message`** — The only input that may trigger
a Component transition. User intent, domain events, EffectOutcomes,
SourceEvents, request outcomes, and behaviorally relevant failures all enter
application logic as Component messages. Concrete names should normally make
the association visible, such as `CounterMessage` or `HealthMessage`.

**Transition / `update`** — One deterministic application of a Component
Message to a Model, producing committed state and explicit Commands.
Transitions for one Component never overlap.

**Practical purity / observational purity** — The requirement that a
transition's observable result depend only on its fixed Component
implementation and configuration, Model, and Message. Exclusively owned
in-place mutation is permitted when it is observationally equivalent to
producing a new Model value.

**Runtime** — The machinery that owns Models, delivers messages, invokes
transitions, interprets Commands, reconciles Subscriptions, supervises work,
and enforces Samara's execution semantics.

**Runtime topology** — The runtime's internal arrangement of tasks, mailboxes,
threads, queues, or loops. Topology is not a public semantic contract unless a
specific observable property is deliberately promised.

## Finite Work and Ongoing Work

**Command / `Command<Message>`** — An inert value describing finite work
requested by a transition. A Command may request an EffectDescriptor, schedule
a Message, communicate with another Component, issue a correlated Request, or
emit a Reply. *Command* and *effect* are therefore not synonyms.

**EffectDescriptor** — An inert, typed description of one finite world-facing
interaction. Each Command occurrence creates a distinct effect invocation,
even when two descriptors contain equal-looking data. EffectDescriptors need
not be comparable or cloneable.

**EffectOutcome** — The single terminal completion of an effect invocation: a
typed success, a failure carrying typed Error data, or cancellation as defined
by that effect's contract. A one-shot message mapper transforms it into a
Component Message.

**Error** — Typed data explaining why an operation could not complete as
intended, such as `TcpError`, `DecodeError`, or `RequestError`. Concrete payload
types use the `Error` noun rather than `Failure`.

**Failure** — The semantic occurrence of an operation failing, normally
carrying Error data. *Failed* is appropriate for an outcome or event variant,
as in `EffectOutcome::Failed(error)`. Normal Source ending and cancellation are
terminal conditions but are not failures.

**SourceDescriptor** — An inert, typed, comparable description of ongoing event
production. Comparability exists so Subscription reconciliation can determine
whether desired work is unchanged; a SourceDescriptor does not contain a live
resource or stable Subscription identity.

**Terminal descriptor** — An EffectDescriptor or SourceDescriptor directly
realized by a matching Driver in live execution or by terminal controlled
behavior. `TcpBytes` is a terminal SourceDescriptor.

**Composed descriptor** — An EffectDescriptor or SourceDescriptor built by one
or more Layers around another descriptor. Interpretation applies those Layers
until it reaches a terminal descriptor. `Framed<TcpBytes, D>` is a composed
SourceDescriptor whose framed event vocabulary is visible to its Subscription.

**Subscription / `Subscription<Message>`** — A Component's declarative desire
to maintain a Source matching a SourceDescriptor under a stable,
Component-local identity, plus a message mapper from SourceEvents to Component
Messages. Declaring a Subscription does not itself start work.

**Source** — The runtime-scoped ongoing realization executing behind a
SourceDescriptor. It may contain operational resources such as tasks, sockets,
and partial framing buffers. A Source can produce zero or more SourceEvents
until it ends, fails, or is canceled.

**SourceEvent** — One repeatable occurrence produced by a Source. A reusable
message mapper transforms each SourceEvent into a Component Message.

**Desired Subscription** — A Subscription returned from the Component's
current Model during reconciliation.

**Active Subscription** — Runtime bookkeeping for desired ongoing work,
including any currently maintained Source.

**Subscription reconciliation** — The post-transition comparison of desired
and active Subscriptions. A new stable identity starts a Source; the same
identity and equal SourceDescriptor retain it; the same identity and a changed
descriptor replace it; a removed identity cancels it. Removal or replacement
does not inherently manufacture a SourceEvent.

**Message mapper** — Pure synchronous application logic that converts boundary
data into a Component Message. EffectOutcome and request-outcome mappers are
one-shot and may be represented by `FnOnce`; SourceEvent and protocol-binding
mappers are reusable and may be represented by `Fn`. Mapper object identity is
not itself part of Command or Subscription semantics.

**Adapter** — The conceptual umbrella for code that translates or realizes a
declared boundary. It is useful in architecture and user-guide prose, but
Samara does not currently require a universal `Adapter` trait or an `Adapter`
suffix on concrete types. The two important categories are Layers and Drivers.

**Compositional Adapter / Layer** — A declarative transformation that stays
inside the Samara boundary and composes one descriptor or event vocabulary into
another. A Layer behaves identically in live and controlled execution. It may
have runtime-scoped deterministic state, such as a framing buffer, but it does
not perform ambient I/O. Samara may eventually have multiple Layer kinds rather
than one universal `Layer` trait.

**Terminal Adapter / Driver** — The code-level abstraction that realizes a
terminal descriptor against a selected live surrounding world. Drivers may use
Tokio and operational resources, but all work remains runtime-scoped and
cancellable. An execution profile supplies Drivers through program assembly;
they are not associated with a Component implementation.

**`EffectDriver<D>`** — The provisional static relationship between a terminal
EffectDescriptor type `D` and its live-world Driver.

**`SourceDriver<D>`** — The provisional static relationship between a terminal
SourceDescriptor type `D` and its live-world Driver.

Controlled execution provides deterministic behavior for the same terminal
descriptor contracts without silently invoking live Drivers. The exact Rust
shape of profile bindings remains open.

The finite and ongoing lifecycles are deliberately similar only where their
semantics are actually similar:

```text
Command + EffectDescriptor
    -> zero or more Layers
    -> terminal EffectDriver in live execution, or controlled behavior
    -> EffectOutcome exactly once
    -> one-shot message mapper
    -> Component Message

Subscription(identity + SourceDescriptor + message mapper)
    -> reconciliation
    -> zero or more Layers
    -> terminal SourceDriver in live execution, or controlled behavior
    -> Source
    -> SourceEvent zero or more times
    -> reusable message mapper
    -> Component Message
```

**Mechanism** — A reusable execution capability owned by the runtime or a
low-level Layer or Driver, such as scheduling, transport I/O, cancellation, or
task supervision.

**Policy** — An application or protocol decision such as retry, reconnect,
framing choice, timeout meaning, or domain error mapping. Policy must not be
hidden in runtime-owned mechanism.

**Structured concurrency / runtime-owned scope** — The rule that every
Samara-authorized asynchronous activity remains owned, cancellable, and
accountable through a runtime scope. Detached work is non-conforming.

## Descriptor Naming

The `EffectDescriptor` and `SourceDescriptor` traits name the architectural
role. Concrete descriptor types should directly name the declared intent, such
as `StoreFrame`, `TcpBytes`, or `Framed<S, D>`, rather than mechanically repeat
the trait name in every type. A `Descriptor` suffix is appropriate only when
the semantic name would otherwise be ambiguous.

Concrete descriptor types must not use `Source` to mean an inert declaration,
because `Source` is reserved for the runtime-scoped realization.
`StreamDescriptor<T>` uses the otherwise optional `Descriptor` suffix to avoid
confusion with a running stream. It names logical event production independently
of the live adapter; `bind_mpsc` names one concrete Tokio realization.

Application aliases for composed descriptors should use intrinsic domain
language when it exists. Names such as `TelemetryFeed` in examples are domain
labels, not additional Samara runtime concepts.

## Component Interaction

**Protocol** — A provider-neutral typed vocabulary exposed through a Port
rather than through a provider Component's private Message type. Qualify
network formats as *wire protocols* to avoid ambiguity.

**Protocol message / `Protocol::Message`** — The complete provider-neutral
typed vocabulary carried through a Protocol's Ports. It includes one-way
Notification variants and dynamic RequestInvocation variants. Concrete enum
names should make the association explicit, such as `HealthProtocolMessage`;
directional names such as `HealthInbound` are avoided.

**Port** — A named, inert logical dependency on a Protocol. A Port contains no
provider reference, channel, runtime handle, or lookup capability.

**Provider** — The concrete Component selected during program assembly to
receive Protocol Messages sent through a Port.

**Requester** — A Component that issues a Request through a Port. It owns the
request continuation and any domain correlation while the runtime owns
transport correlation.

**Port binding** — The assembly-time mapping from one exact named Port to a
provider and its private Component Message vocabulary. The provider's Message
implements `From<Protocol::Message>`, making the pure vocabulary conversion a
standard type relationship rather than a closure repeated at each binding.
Multiple named Ports of the same Protocol may still be bound independently.

**Notification** — A one-way Protocol value implementing `Notification<P>` and
sent through `Command::notify`. It has no correlated terminal outcome for the
sender. Notification delivery-failure policy remains unresolved.

**Request** — A Protocol operation/value implementing `Request<P>` with a
statically associated `Reply` type. `Command::request` issues it as finite
correlated work. Its eventual typed RequestOutcome is transformed by a request
continuation into an ordinary requester Component Message; the requester does
not await inside `update`.

**RequestInvocation** — One dynamic Request occurrence delivered to the
provider. It contains the Request value and a one-shot `ReplyTo` authority. The
runtime, not application code, associates the invocation with its eventual
outcome.

**Reply** — The successful response type statically associated with a Request.
A provider emits a value of that type using the invocation's `ReplyTo`
authority.

**Request outcome / `RequestOutcome`** — The requester-visible single terminal
outcome of a Request, provisionally a Reply, a failure carrying runtime Error
data, timeout, or cancellation. The exact failure and deadline policy is not
yet settled.

**`RequestError`** — The current provisional type for a Request's runtime-level
terminal error data. Domain-level negative replies remain ordinary Reply
values.

**Request continuation** — The one-shot message mapper attached to a Request
invocation. It states what the separated RequestOutcome means to the requester,
may capture domain context, and produces the requester's Component Message.

**Transport correlation** — Opaque runtime bookkeeping that distinguishes one
dynamic RequestInvocation from every other invocation. Components neither
create nor compare transport-correlation identifiers.

**Domain correlation** — Application-owned identity that explains the business
meaning of a Request or Reply. It is commonly captured by the request
continuation and returned in the resulting Component Message.

**`ReplyTo`** — The current provisional name for an opaque, one-shot authority
that permits a provider to emit the correctly typed Reply. It is inert data,
not a channel, future, or runtime handle.

**`ComponentRef`** — The current provisional name for an inert typed logical
address used when direct coupling to another Component's complete Message API
is deliberate.

**`ComponentHandle`** — The current provisional name for a live
external-ingress capability available at the Samara program boundary. Unlike a
`ComponentRef`, it is a runtime capability and must not be available inside
Components.

## Execution and Ordering

**Live execution** — Execution using real Tokio scheduling, time, I/O, and
external systems. It preserves Samara's Component and causal guarantees but
does not deterministically order independent events.

**Controlled execution** — Execution in a closed, runtime-owned world where
inputs, effects, time, randomness, identifiers, and scheduling decisions are
controlled or seeded reproducibly. It is the guarantee-bearing term;
simulation is one use case for it.

**Controlled world** — The scripted or seeded substitute for the surrounding
world used during controlled execution. Missing controlled behavior fails
explicitly rather than falling back to live behavior.

**Logical time / controlled time** — Runtime-controlled time that can advance
manually or automatically without wall-clock sleeping.

**Runtime semantics** — The observable scheduling, lifecycle, and tie-breaking
rules whose sameness is part of a controlled-determinism claim.

**Transition determinism** — For a fixed conforming Component implementation
and configuration, equivalent Model and Component Message inputs produce
equivalent next state and Commands.

**Program-wide controlled determinism** — Identical program, controlled world,
inputs, seeds, and runtime semantics produce the same whole-program semantic
trace, final state, causal relationships, and scheduled future work.

**Per-Component serialization** — A Component's transitions never overlap and
form one local history. This does not imply FIFO delivery from every Source or
a program-wide order.

**Causal edge / causal ordering** — A required predecessor relationship
established by the program, such as Command-to-Outcome, send-to-delivery, or
Request-to-Reply.

**Independent events** — Events with no causal or explicitly sequenced
relationship. Samara promises no relative live order between them.

**Explicit FIFO / explicit sequencing** — Ordering guaranteed by a particular
delivery or work contract rather than inferred from runtime topology.

**Global total order** — One comparable sequence containing every program
event. Samara explicitly does not promise this for independent live events.

**Quiescence** — The absence of immediately runnable work. Work may still be
waiting for logical time, a controlled input, or an external event.

## Observability and Correctness

**In-band semantic result** — Behaviorally relevant data delivered to a
Component as a Message. Components can make decisions from it.

**Semantic trace** — An out-of-band structured record of transitions,
descriptors, lifecycle results, time, and causation. Observing the trace cannot
feed data back into Components or otherwise alter semantic behavior.

**Runtime diagnostic** — Operational information such as task identity, thread
placement, queue depth, or incidental sequence number. It is not an application
semantic contract unless explicitly promoted into one.

**Conformance** — The combined obligations of the runtime and of Component,
Layer, Driver, and controlled-behavior authors required for Samara's guarantees
to hold.

## Distinctions Worth Preserving

| Do not collapse | Distinction |
| --- | --- |
| Command and EffectDescriptor | A Command is the broader finite-work envelope; an EffectDescriptor is one typed world-facing intent it may contain. |
| EffectOutcome and SourceEvent | An EffectOutcome terminates one invocation; a SourceEvent is one of zero or more occurrences from ongoing work. |
| SourceDescriptor, Subscription, and Source | A SourceDescriptor describes ongoing production, a Subscription adds desire, identity, and mapping, and a Source is the runtime-scoped realization. |
| Layer and Driver | A Layer composes declarations inside the program boundary; a Driver terminates a descriptor into the selected live world. |
| Adapter and a code abstraction | Adapter is the conceptual category; Layer and Driver are the code-level roles Samara currently names. |
| Component Message and Protocol Message | A Component Message is private transition input; a Protocol Message is the provider-neutral vocabulary crossing a Port. |
| Request and RequestInvocation | A Request is the typed operation/value; a RequestInvocation is one dynamic occurrence carrying Request and `ReplyTo`. |
| Port and provider | A Port is an inert dependency; the provider is the Component selected during assembly. |
| Notification and Request | `Notification<P>` is one-way input issued by `Command::notify`; `Request<P>` has an associated Reply and is issued by `Command::request` with a continuation. |
| Protocol and wire protocol | A Samara Protocol is a typed Component boundary; a wire protocol defines external data exchange. |
| Message and event | Boundary events and outcomes are mapped into Component Messages; trace events remain out of band. |
| Per-Component serialization and ordering | Non-overlapping transitions do not create a global order or imply unspecified FIFO behavior. |
| Controlled execution and simulation | Controlled execution names the semantic profile; simulation is one workload that uses it. |
| Domain and transport correlation | Applications own meaning; the runtime owns delivery bookkeeping. |
| `ComponentRef` and `ComponentHandle` | A reference is inert logical wiring; a handle is a live ingress capability. |

## Historical or Avoided Vocabulary

| Historical or ambiguous term | Current vocabulary | Guidance |
| --- | --- | --- |
| Actor | Component | Use *Component* for Samara's topology-neutral application unit. Actor language survives in older PoC documents. |
| Component value | Component configuration | Name the role rather than implying a second application unit. |
| `Msg` | Message / `Component::Message` | Use *Component Message* when it must be distinguished from a Protocol Message. |
| `Cmd` | Command | Use the full word in type and method names. |
| Effect value / Effect intent | EffectDescriptor | Use *effect* generically for the interaction, not as a second ambiguous type noun. |
| `EffectEvent` | EffectOutcome | An effect has one terminal outcome; repeatable occurrences are SourceEvents. |
| Error payload types named `*Failure`, such as `TcpFailure` | `*Error`, such as `TcpError` | Error names explanatory data; failure names the semantic occurrence carrying it. |
| Result mapper | Message mapper | Name what the pure function produces. A request's one-shot mapper is its request continuation. |
| Source meaning a descriptor | SourceDescriptor | Reserve *Source* for the runtime-scoped ongoing realization. |
| `MpscSource<T>`, `MpscInput<T>` | `StreamDescriptor<T>` | Name the inert logical stream independently of its live adapter; reserve Source for its runtime realization. |
| `Incoming<P, R>` | `RequestInvocation<P, R>` | Name the dynamic Request occurrence, not merely its direction. |
| `Protocol::Inbound`, `HealthInbound` | `Protocol::Message`, `HealthProtocolMessage` | Name what the enum contains and associate concrete names with their Protocol. |
| `ActorRef`, `Addr`, `PortRef`, `RuntimeRef` | `ComponentRef`, `ComponentHandle`, or `Port` | Choose the term that distinguishes logical address, live ingress capability, or dependency. |
| `ask` | `request` | Use `Command::request` for command-now, Message-later request/reply; *ask* can misleadingly suggest an awaitable future. |
| `tell` | `notify` or `send` | Use *notify* for provider-neutral one-way Protocol interaction and *send* for deliberate direct delivery to a Component Message API. |
| `ReplyToken` | `ReplyTo` | `ReplyTo` is the current provisional spelling; do not canonize both. |
| Simulated execution | Controlled execution | Use *controlled* for the semantic profile; use *simulation* for the workload or product use case. |
| Mailbox loop | Runtime topology | A mailbox or event loop may be an implementation technique, not the definition of a Component. |
| Effect handler, interpreter, backend, source adapter | Driver or controlled behavior, qualified by role | Use *Driver* for a terminal live-world code boundary, *Layer* for declarative composition, and *Adapter* only as the umbrella concept. Controlled execution supplies behavior without falling through to live Drivers. |

## Open API Questions

These questions are intentionally recorded rather than answered by the Phase 2
Component-kernel freeze:

- The exact RequestOutcome variants and the deadline, cancellation,
  late-Reply, and abandoned-Reply policies.
- Notification delivery-failure semantics.
- The initial code shape of Layer abstractions. Multiple Layer kinds are
  expected, so this checkpoint does not promise one universal `Layer` trait.
- Whether a newly declared message mapper replaces the prior mapper while the
  same Subscription identity and equal SourceDescriptor retain their running
  Source. Mapper object identity is not semantic, but mapper behavior is.
- The exact Rust representation of live and controlled execution-profile
  bindings. Controlled execution must preserve the same descriptor contracts,
  but it need not execute the live Driver traits.
- Structured shutdown's drain-versus-cancel policy.
