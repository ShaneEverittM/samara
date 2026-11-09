# Samara Glossary

- Status: Draft
- Date: July 25, 2026
- Scope: Canonical project vocabulary and important distinctions

This document defines how Samara currently uses its growing vocabulary. It is a
reference for API design, implementation, testing, and documentation—not a
substitute for the behavioral contracts in the vision and ADRs.

The concepts below are canonical unless marked historical. Accepted API slices
and the Phase 5 controlled-execution contract follow these spellings. Accepted
ADR-0004 uses them for the active initial live-runtime contract.
ADR-0008 closes Program assembly around Program-issued Component, Port,
Effect, and Source capabilities.
`api-contract.md` records which slice is frozen for each
implementation phase and which policy-bearing surfaces remain provisional.

## Program and State

**Samara program** — A closed declared collection of Components and the
Component, Port, Effect, and Source capabilities they may use. It contains
logical application structure, not live Tokio resources. Execution can issue
new occurrences and Model-derived descriptor values but cannot add another
dependency after build.

**Program assembly** — The construction of a Samara program: Component
registration, issuance of ComponentRefs, Ports, and Effect and Source
capabilities, Port binding, and profile selection. `ProgramBuilder::build()`
rejects logical assembly errors and closes the capability inventory; the
profile builders then validate every declared terminal requirement before
execution (ADR-0008).

**Assembly** — A reusable slice of Program assembly: a type whose `install`
declares its own Components, Ports, and capabilities into an enclosing
`ProgramBuilder`, exports what callers may use (normally a Port), and lists
the world boundaries the host must drive as `Requires`. The unit of reuse in
Samara is an Assembly, not a Component. It leaves no trace at runtime: the
Components it installs are ordinary peers reached through Ports. Not a
*SubComponent*: Samara has no parent-child `update` nesting.

**Requires (of an Assembly)** — The type-level list of `Effect<D>` and
`Source<S>` markers naming what an Assembly needs driven. The live builder's
`with_assembly(installed).and_drivers(..)` takes a matching tuple of Drivers,
position by position; the controlled builder's `with_assembly(installed)`
takes control of every entry and needs nothing else.

**VendorsDrivers** — An optional trait an Assembly implements to ship concrete
live Drivers for its `Requires`, bound with `and_its_vendored_drivers()`. A
library normally feature-gates the impl so consumers bringing their own
Drivers do not pay for the vendored ones.

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

**Component kind** — The Rust type that implements `Component`. It is only a
name: no value of it is ever stored, and `init`, `update`, and
`subscriptions` are associated functions on it (ADR-0013). `ComponentRef<C>`
and `component::<C>(..)` name the kind.

**Component configuration / `Component::Config`** — The immutable value a
kind is registered with. It holds logical wiring and declarative
configuration, such as Ports, EffectCapabilities, SourceCapabilities, and
SourceDescriptors, and is passed to every Component function by shared
reference, signaling that it is observationally read-only during execution.
Because Rust permits interior mutability, preserving that property is also a
conformance obligation. Behaviorally relevant mutable state does not belong
here.

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
requested by a transition: an effect, a scheduled Message, a send to another
Component, a correlated Request, or a Reply. Constructing and discarding one
requests no work. *Command* and *effect* are not synonyms.

**EffectDescriptor** — An inert, typed description of one finite world-facing
interaction. Each Command occurrence is a distinct invocation even when two
descriptors hold equal data, so descriptors need not be comparable or
cloneable.

**EffectCapability / `EffectCapability<D>`** — An inert Program-issued value
declaring that its Program may issue descriptor type `D`. Required by every
Effect Command constructor; performs no I/O and holds no handle. Cloning it
declares nothing new, and its private provenance stops it authorizing work in
another Program.

**EffectOutcome** — The single terminal completion of an effect invocation:
typed success, failure carrying typed Error data, or cancellation as the
effect's contract defines. A Command may instead declare that it discards the
outcome; that is still runtime-owned finite work, not fire and forget. Aborting
the whole live scope is ownership cleanup and manufactures no outcome
(ADR-0004).

**HttpRequest / HttpResponse / HttpError** — The first-party raw HTTP boundary
(ADR-0005). The descriptor owns method, URL, headers, and body; the response is
the complete buffered status, headers, and body, and an error status is still
a response; `HttpError` is configuration or transport failure, never a remote
status.

**HttpResponsePipeline** — The must-use pure continuation `on_response`
produces from an `HttpRequest`. It declares status policy and decoding and
lowers to the same raw effect plus a composed mapper; it is not a Driver,
Layer, or separately traced effect. Its error algebra, `HttpResponseError`,
distinguishes transport `HttpError`, an explicit `HttpStatusError` rejection
that retains the response, and `HttpJsonError`, which retains the response and
the decode error. Cancellation is not a variant.

**StdinLines / StdinError** — The first-party terminal SourceDescriptor for
UTF-8 lines from process stdin (ADR-0009), with line-ending stripping and one
final unterminated Item at EOF. `StdinError` is a typed `Read` or
`InvalidUtf8` failure that ends the Source without `Ended`. The descriptor is
platform-neutral; `bind_stdin` is the Unix-only exact live binding of the sole
declared stdin capability, one per runtime, sharing one process-wide lease.

**Error** — Typed data explaining why an operation could not complete, such as
`TcpError` or `RequestError`. Payload types use the `Error` noun.

**Failure** — The semantic occurrence of an operation failing, normally
carrying Error data, as in `EffectOutcome::Failed(error)`. Normal Source ending
and cancellation are terminal conditions, not failures.

**SourceDescriptor** — An inert, typed, comparable description of ongoing event
production. Comparability exists so reconciliation can tell whether desired
work is unchanged; it holds no live resource and no Subscription identity.

**SourceCapability / `SourceCapability<S>`** — An inert Program-issued value
declaring that its Program may subscribe to descriptor type `S`. For a
composed `S` it also records the lowered terminal requirement, so applications
never declare inner descriptors or Layers separately.

**Terminal descriptor** — A descriptor realized directly by a Driver or by
terminal controlled behavior. `TcpBytes` is one.

**Composed descriptor** — A descriptor built by one or more Layers around
another. Interpretation applies the Layers until it reaches a terminal
descriptor. `Framed<TcpBytes, D>` is one.

**SourcePlan** — The runtime-owned result of lowering a composed
SourceDescriptor during reconciliation: terminal descriptor, ordered Layers,
capability identity, and mapper. Not an application declaration, identity, or
running Source.

**Subscription / `Subscription<Message>`** — A Component's declarative desire
to maintain a Source for a SourceDescriptor under a stable Component-local
identity, through a SourceCapability, with a reusable mapper from SourceEvents
to Messages. Declaring one starts nothing.

**Source** — The runtime-scoped ongoing realization behind a SourceDescriptor.
It may own tasks, sockets, and partial buffers, and produces zero or more
SourceEvents until it ends, fails, or is cancelled.

**Source generation** — Private runtime identity for one Source realization.
Replacement withdraws the old generation atomically; its stale events are
discarded and traced. Applications that need to observe generations carry
their own.

**SourceEvent** — One repeatable occurrence produced by a Source, mapped into a
Component Message each time.

**Source terminal arbitration** — Exactly one terminal condition wins for an
active live Source: an accepted `end`, an accepted `fail`, or Driver return;
cancellation, replacement, shutdown, or fault winning first suppresses later
terminal events (ADR-0004).

**Desired Subscription / Active Subscription** — What the Component's current
Model asks for, and the runtime's bookkeeping for what it currently maintains.

**Subscription reconciliation** — The post-transition comparison of desired and
active Subscriptions: a new identity starts a Source; the same identity,
capability, and equal descriptor retain it and install the latest mapper; a
changed capability or descriptor replaces it with a hard generation cutover; a
removed identity cancels it. Removal and replacement manufacture no
SourceEvent.

**Message mapper** — Pure synchronous application logic converting boundary
data into a Component Message. Outcome mappers are one-shot (`FnOnce`); event
and protocol mappers are reusable (`Fn`). Mapper identity is not part of
Command or Subscription semantics.

**Adapter** — The conceptual umbrella for code that translates or realizes a
declared boundary. No universal trait; the two categories that matter are
Layers and Drivers.

**Layer** — A compositional adapter: a declarative transformation of one
descriptor or event vocabulary into another, identical in live and controlled
execution, holding at most deterministic state such as a framing buffer, and
performing no ambient I/O. More than one Layer kind may exist.

**Driver** — A terminal adapter: the code that realizes a terminal descriptor
against the live world, under runtime scope and cancellation, supplied by
assembly rather than attached to a Component. `EffectDriver<D>` and
`SourceDriver<D>` are the two roles. A normal return maps to exactly one
outcome or terminal event; a panic faults the runtime instead of inventing an
application result.

**Type-wide binding** — One Driver or controlled registration satisfying every
capability that lowers to the same terminal descriptor type.

**Exact binding** — A binding tied to one capability identity because it owns
one concrete resource, such as a one-shot `mpsc` receiver or process stdin.

**Mechanism** — A reusable execution capability owned by the runtime or a
low-level Layer or Driver: scheduling, transport I/O, cancellation, task
supervision.

**Policy** — An application or protocol decision: retry, reconnect, framing
choice, timeout meaning, domain error mapping. Policy must not hide in
mechanism.

**Structured concurrency / runtime-owned scope** — Every Samara-authorized
asynchronous activity stays owned, cancellable, and accountable through a
runtime scope. Detached work is non-conforming.

**Decoder finalization** — The pure EOF operation a Decoder performs once when
its underlying Source ends normally, emitting final frames before `Ended` or
one decode failure without `Ended`. Failure, replacement, cancellation,
shutdown, and fault do not invoke it (ADR-0004).

The two lifecycles are parallel only where their semantics are:

```text
Command + EffectCapability + EffectDescriptor
    -> zero or more Layers -> terminal Driver or controlled behavior
    -> EffectOutcome exactly once (whole-scope abort may produce none)
    -> one-shot mapper -> Message, or discarded -> no Message

Subscription(identity + SourceCapability + SourceDescriptor + mapper)
    -> reconciliation -> SourcePlan -> terminal Driver or controlled behavior
    -> Source -> SourceEvent zero or more times
    -> reusable mapper -> Message
```

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
of the live adapter; `bind_mpsc(&capability, receiver)` names one concrete Tokio
realization for one exact SourceCapability whose terminal descriptor is
`StreamDescriptor<T>`. That capability may itself describe a built-in composed
Source, in which case stream items still traverse its Layers.

The accepted first-party `mpsc` realization is one-shot because a Tokio
receiver is a unique live resource. Channel closure ends its Source normally;
duplicate activation or reactivation after consumption or cancellation faults
explicitly rather than implying a recreated receiver.

`StdinLines` directly names the intrinsic line-oriented intent, so it needs no
`Descriptor` suffix. Unix `bind_stdin()` names the unique live
process resource. It removes CRLF or LF framing, preserves empty lines, emits a
final unterminated line before clean EOF, and reports read or UTF-8 failure as
typed Source Error data. Private readiness, cancellation, and reader-thread
failures remain runtime faults. Controlled execution scripts the same terminal
event vocabulary without opening stdin.

Application aliases for composed descriptors should use intrinsic domain
language when it exists. Names such as `TelemetryFeed` in examples are domain
labels, not additional Samara runtime concepts.

## Component Interaction

**Port message type** — The message enum a `Port<M>` or `Connector<M>`
carries: the complete vocabulary of one dependency, written by hand. A
one-way variant carries only its data; a request variant also carries a
`ReplyWith<R>`, which is what types its reply. Name the enum for what it
contains, such as `HealthMessage`; directional names such as `HealthInbound`
are avoided. The word *protocol* is reserved for wire protocols.

**Port** — A consumer-owned, named, inert Program-issued dependency on a
message type: `Port<M>` is a hole the consuming application cuts in its own
vocabulary and fills at assembly by connecting a provider. A Port contains no
provider reference, channel, runtime handle, or lookup capability. It remains
distinct from the live-only `PortHandle` obtained from an assembled runtime.
This is the hexagonal *port*; Elm uses the same word for JavaScript interop,
which Samara does not have.

**Connector** — A provider-owned, inert address: a `ComponentRef` erased to the
Message type it accepts, `Connector<M>`, together with the pure conversion from `M`
into the Component's private Message. A Component or Assembly hands out a Connector
to expose itself without naming its Component kind; the holder connects it to
one of its own Ports with `connect` after adapting it with `accepting`,
writing the coupling between the two vocabularies once at that site, or
holds it directly and addresses it with
`Command::send` and `Command::request_to`, whose request is a constructor
function the provider published taking `ReplyWith<R>`. `Connector::accepting`
adapts a Connector to the holder's vocabulary. Like `ComponentRef` and `Port`, a
Connector grants no delivery capability.

**Provider** — The Component that receives what is sent through a Port or to
a Connector: selected at program assembly for a Port, named by the Connector
otherwise.

**Requester** — The origin of a Request through a Port. A Component requester
owns a Message continuation and any domain correlation. A live host requester
awaits its Reply through `PortHandle`. In both cases the runtime owns transport
correlation.

**Port binding** — The assembly-time mapping from one exact named Port to a
`Connector<M>` of the same message type, made with
`ProgramBuilder::connect(&port, &connector)`. A Component provides the
Connector: `ComponentRef::connector()` when its Message is `M` or implements
`From<M>`, or `accepting(map)` with the pure conversion, normally the enum
variant that wraps the Port message. A library Connector in another
vocabulary is adapted the same way with `Connector::accepting`. Multiple
named Ports of the same message type may still be bound independently.

**Notification** — Informal name for a one-way message: a variant with no
reply token, delivered with `Command::send` or `PortHandle::send`. The sender
gets no correlated outcome. There is no trait; the word only distinguishes it
from a request in prose.

**Request** — A message variant carrying a `ReplyWith<R>`. A Component issues
one with `Command::request_to(target, constructor, map)`, passing the
variant's constructor; the runtime mints the token, the constructor's
`ReplyWith<R>` fixes the reply type, and the typed `RequestOutcome<R>` arrives
later through `map` as an ordinary Message. A live host issues the same
constructor through `PortHandle::request_to` and awaits `R` directly
(ADR-0016).

**Reply** — The successful response type `R` named by a request variant's
`ReplyWith<R>`. A provider emits a value of that type with `Command::reply`.

**Request outcome / `RequestOutcome`** — The single terminal result of a
Component-issued Request, mapped to a requester Message: `Replied`, or
`TimedOut` for the opt-in timeout forms (ADR-0011). `Failed` and `Cancelled`
are reserved.

**`RequestError`** — The current provisional type for a Request's runtime-level
terminal error data. Domain-level negative replies remain ordinary Reply
values.

**Request continuation** — The one-shot Message mapper passed to
`Command::request_to`, `Message::from` when one reply has one meaning or a
closure that captures domain context. A live host request has a waiter
instead.

**Transport correlation** — Opaque runtime bookkeeping that distinguishes one
dynamic request from every other. Components and live host
callers neither create nor compare transport-correlation identifiers.

**Domain correlation** — Application-owned identity that explains the business
meaning of a Request or Reply. It is commonly captured by the request
continuation and returned in the resulting Component Message.

**`ReplyWith<R>`** — The opaque, one-shot authority carried by a request
variant that obliges and permits its holder to reply with an `R`. It is inert
data, not a channel, future, or runtime handle; `Command::reply` consumes it.

**`ComponentRef`** — The current provisional name for an inert typed logical
address issued during Program assembly when direct coupling to another
Component's complete Message API is deliberate.

**`ComponentHandle`** — A live external-ingress capability for one Component,
available only at the program boundary and never inside Components.
Successful `send` means accepted for delivery, not transition completion.

**`PortHandle`** — A cloneable live external-ingress capability for one bound
`Port<M>`, admitting sends and requests through the normal Port binding
without exposing the provider. A host request awaits its
Reply directly; closure or fault wakes it with `RuntimeError`. Live-only; it
has no ControlledRuntime counterpart (ADR-0006).

**`RuntimeTask`** — The live structured-ownership handle from
`LiveRuntime::spawn`. `run_forever` observes termination without initiating
shutdown; `shutdown` selects Drain or Cancel explicitly (ADR-0007, ADR-0010).

## Execution and Ordering

**Live execution** — Execution using real Tokio scheduling, time, I/O, and
external systems. It preserves Samara's Component and causal guarantees but
does not deterministically order independent events.

**Controlled execution** — Execution in a closed, runtime-owned world where
inputs, effects, time, randomness, identifiers, and scheduling decisions are
controlled or seeded reproducibly. It is the guarantee-bearing term;
simulation is one use case for it.

**Controlled world** — The scripted or seeded substitute for the surrounding
world in controlled execution. Missing controlled behavior faults the run
explicitly rather than falling back to a live Driver; state and trace remain
inspectable.

**Logical time / controlled time** — Runtime-controlled time that can advance
manually or automatically without wall-clock sleeping.

**Runtime semantics** — The observable scheduling, lifecycle, and tie-breaking
rules whose sameness is part of a controlled-determinism claim.

**Deterministic insertion ticket** — The stable secondary key that orders
equal-time controlled work after logical deadline (ADR-0003). Reproducibility
machinery, not live or domain order.

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

**Admission** — The point at which live boundary input becomes runtime-owned
work, through `ComponentHandle::send`, `SourceSink`, or a `PortHandle`
operation. A call racing shutdown is accepted or rejected explicitly; accepted
work is not silently dropped for capacity (ADR-0004, ADR-0006).

**Unbounded internal delivery** — The v0 live mechanism after admission: no
configurable capacity and no overload drop, so memory can grow without bound
under sustained pressure. A documented first-cut limitation, not a throughput
or backpressure promise.

**Drain shutdown / `Shutdown::Drain`** — Closes ingress, stops Sources, and
processes accepted and causally emitted finite work to completion, with no
implicit deadline.

**Cancel shutdown / `Shutdown::Cancel`** — Closes ingress, stops driving,
cancels obligations and owned tasks, and joins or aborts everything without
manufacturing application outcomes. Host waiters wake with `RuntimeError`.

**Quiescence** — The absence of immediately runnable work. Work may still be
waiting for logical time, a controlled input, or an external event.

**`pending_now`** — The count of semantic obligations ready during controlled
execution: accepted Component Messages and due timers.

**`pending_later`** — The count of semantic obligations awaiting an outcome,
event, or future logical instant: pending effects, future timers, active Sources
(one each), and outstanding Requests. Runtime tasks, queues, locks,
interpreter steps, and trace records are not additional work units.

## Observability and Correctness

**In-band semantic result** — Behaviorally relevant data delivered to a
Component as a Message. Components can make decisions from it.

**Semantic trace** — An out-of-band structured record of transitions,
descriptor types and targets, lifecycle results, time, and causation. Controlled
execution always collects the v0 trace in memory for tests to read after
driving. It is structural rather than domain-payload-complete; typed intent
tests compare descriptor and Message payloads when needed.

**Trace record / `TraceRecord`** — One controlled semantic-trace entry with a
`TraceId`, `LogicalTime`, optional immediate cause, and one `TraceEvent`.
Initialization and controlled harness inputs are roots with no parent; every
other record has exactly one immediate causal parent. The trace includes stale
Source-generation drops. Its compatibility follows ordinary crate API
versioning; v0 defines no durable storage schema.

**Runtime diagnostic** — Operational information such as task identity, thread
placement, queue depth, or incidental sequence number. It is not an application
semantic contract unless explicitly promoted into one.

**Runtime fault** — An execution failure that cannot truthfully be represented
as typed application data, such as a Driver panic or exhausted one-shot bridge.
Under ADR-0004, a live runtime fault closes ingress, stops application
driving, triggers structured cancellation, and surfaces `RuntimeError` to the
host after owned work is closed.

**Conformance** — The combined obligations of the runtime and of Component,
Layer, Driver, and controlled-behavior authors required for Samara's guarantees
to hold.

## Distinctions Worth Preserving

| Do not collapse | Distinction |
| --- | --- |
| Command and EffectDescriptor | A Command is the broader finite-work envelope; an EffectDescriptor is one typed world-facing intent it may contain. |
| EffectCapability and EffectDescriptor | The capability declares one closed Program dependency; each descriptor carries one concrete finite intent. |
| EffectCapability or SourceCapability and a live Handle | Descriptor capabilities are inert Component wiring that only authorizes declarative work; ComponentHandle and PortHandle cross live external ingress. |
| EffectOutcome and SourceEvent | An EffectOutcome terminates one invocation; a SourceEvent is one of zero or more occurrences from ongoing work. |
| SourceCapability, SourceDescriptor, Subscription, SourcePlan, and Source | A SourceCapability declares one closed Program dependency, a SourceDescriptor describes concrete ongoing production, a Subscription adds desire, identity, and mapping, a SourcePlan lowers that desire to ordered Layers and a terminal descriptor, and a Source is the runtime-scoped realization. |
| Layer and Driver | A Layer composes declarations inside the program boundary; a Driver terminates a descriptor into the selected live world. |
| Adapter and a code abstraction | Adapter is the conceptual category; Layer and Driver are the code-level roles Samara currently names. |
| Component Message and Port message | A Component Message is private transition input; a Port message is a value of the provider-neutral vocabulary crossing a Port. A library written the Elm way makes one opaque type serve as both. |
| Request and reply token | A request is a message variant; its `ReplyWith<R>` field is the one-shot authority to answer it and the type that fixes the reply. |
| Port and provider | A Port is an inert dependency; the provider is the Component selected during assembly. |
| Send and request | `Command::send` delivers a variant with no reply token and gets no outcome; `Command::request_to` delivers one built around a `ReplyWith<R>` and maps the `RequestOutcome<R>` through its continuation. `PortHandle::send` and `PortHandle::request_to` are the live host's forms, the latter with a direct waiter. |
| Protocol and wire protocol | A Samara Protocol is a typed Component boundary; a wire protocol defines external data exchange. |
| Message and event | Boundary events and outcomes are mapped into Component Messages; trace events remain out of band. |
| Per-Component serialization and ordering | Non-overlapping transitions do not create a global order or imply unspecified FIFO behavior. |
| Controlled execution and simulation | Controlled execution names the semantic profile; simulation is one workload that uses it. |
| Domain and transport correlation | Applications own meaning; the runtime owns delivery bookkeeping. |
| `ComponentRef` and `ComponentHandle` | A reference is inert logical wiring; a handle is a live private-Message ingress capability. |
| `Port` and `PortHandle` | A Port is inert provider-neutral logical wiring shared by runtime profiles; a PortHandle is a live-only external-ingress capability for that exact binding. |

## Historical or Avoided Vocabulary

| Historical or ambiguous term | Current vocabulary | Guidance |
| --- | --- | --- |
| Actor | Component | Use *Component* for Samara's topology-neutral application unit. Actor language survives in older PoC documents. |
| Component value, Component implementation | Component kind, Component configuration | The implementing type is a name; the configuration is the value it is registered with. |
| `Msg` | Message / `Component::Message` | Use *Component Message* when it must be distinguished from a Port message. |
| `Cmd` | Command | Use the full word in type and method names. |
| Effect value / Effect intent | EffectDescriptor | Use *effect* generically for the interaction, not as a second ambiguous type noun. |
| `EffectEvent` | EffectOutcome | An accepted EffectOutcome is terminal and occurs once; whole-scope abort may close ownership without one, while repeatable occurrences are SourceEvents. |
| Error payload types named `*Failure`, such as `TcpFailure` | `*Error`, such as `TcpError` | Error names explanatory data; failure names the semantic occurrence carrying it. |
| Result mapper | Message mapper | Name what the pure function produces. A request's one-shot mapper is its request continuation. |
| Source meaning a descriptor | SourceDescriptor | Reserve *Source* for the runtime-scoped ongoing realization. |
| `MpscSource<T>`, `MpscInput<T>` | `StreamDescriptor<T>` | Name the inert logical stream independently of its live adapter; reserve Source for its runtime realization. |
| `Incoming<P, R>`, `RequestInvocation<M, R>` | a request variant carrying `ReplyWith<R>` | The dynamic request occurrence is the message itself (ADR-0016). |
| Protocol, `Protocol::Message`, `protocol!`, `HealthProtocolMessage` | Port message type, `messages!`, `HealthMessage` | Removed November 8, 2026 (ADR-0015). The marker type named one associated type and nothing else; a Port is typed by the message type it carries. ADRs 0006 to 0014 keep the old word. |
| `Protocol::Inbound`, `HealthInbound` | `HealthMessage` | Name what the enum contains, not its direction. |
| `ActorRef`, `Addr`, `PortRef`, `RuntimeRef` | `ComponentRef`, `Connector`, `ComponentHandle`, `Port`, or `PortHandle` | Choose the term that distinguishes kind-typed logical address, Message-typed logical address, live private-Message ingress, inert consumer-owned dependency, or live Port ingress. |
| Jack, `JackId`, `JackHandle`, `plug_in`, Plug, `Pluggable` | Port, `PortId`, `PortHandle`, `connect`, Connector | ADR-0014's names, replaced the same day by ADR-0015 with domain-neutral words. `bind_port` was the pre-0014 spelling of `connect`. |
| `Connectable`, `connect_with`, `connector_with`, `to_connector` | `connect(&port, &connector)`, `ComponentRef::connector`, `accepting` | A Port takes a Connector and nothing else (ADR-0015 amendment, November 8, 2026); the conversion lives on the Connector side, under one verb. |
| Recipient, Address | Connector | Working names while the provider-owned address was designed; *Connector* pairs with *Port* and names the thing held rather than a role. |
| `Installed<A>` | `InstalledAssembly<A>` | Renamed November 8, 2026 so `with_assembly(installed_assembly)` reads as a sentence; the token keeps the `must_use` nudge and `.exports()`. |
| `messages!`, `Notification<M>`, `Request<M>`, `RequestInvocation`, `Command::notify`, `request`, `request_with`, `request_timeout`, `request_timeout_with`, `TraceCommandKind::Notify` | `Command::send`, `Command::request_to`, `request_to_timeout`; a request variant carrying `ReplyWith<R>`; `TraceCommandKind::Send` with `target_port` | Removed November 8, 2026 (ADR-0016). The trait form and the constructor form gave the same guarantee twice; the message type is the protocol. |
| `ask` | `request` | Use `Command::request` for command-now, Message-later Component request/reply and `PortHandle::request` for the deliberately awaitable live host boundary. |
| `tell`, `notify` | `send` | One-way delivery to a Port, Connector, or ComponentRef is `send`; *notify* survives only as prose for a variant without a reply token. |
| `ReplyToken`, `ReplyTo` | `ReplyWith` | Renamed November 8, 2026: in a variant, `Total(ReplyWith<u64>)` states the obligation, reply with a `u64`, where `ReplyTo` named a destination only the runtime knows. |
| Simulated execution | Controlled execution | Use *controlled* for the semantic profile; use *simulation* for the workload or product use case. |
| Mailbox loop | Runtime topology | A mailbox or event loop may be an implementation technique, not the definition of a Component. |
| Effect handler, interpreter, backend, source adapter | Driver or controlled behavior, qualified by role | Use *Driver* for a terminal live-world code boundary, *Layer* for declarative composition, and *Adapter* only as the umbrella concept. Controlled execution supplies behavior without falling through to live Drivers. |
