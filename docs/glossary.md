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
ADR-0008 closes Program assembly around Program-issued Component, Protocol,
Effect, and Source capabilities.
`api-contract.md` records which slice is frozen for each
implementation phase and which policy-bearing surfaces remain provisional.

## Program and State

**Samara program** — A closed declared collection of Components and the
Component, Protocol, Effect, and Source capabilities they may use. It contains
logical application structure, not live Tokio resources. Execution can issue
new occurrences and Model-derived descriptor values but cannot add another
dependency after build.

**Program assembly** — The construction of a Samara program, including
Component registration; ComponentRef, named Port, EffectCapability, and
SourceCapability issuance; provider binding; and execution profile selection.

`ProgramBuilder::build()` rejects logical assembly errors and closes the
Program-issued capability inventory. Live and controlled profile builders then
validate every declared terminal Effect and Source requirement before
execution. The inventory is closed even though assembly does not introspect
arbitrary Component fields or statically enumerate behavior-dependent message
edges.

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
such as Ports, EffectCapabilities, SourceCapabilities, and SourceDescriptors.
Behaviorally relevant mutable state does not belong here.

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
emit a Reply. Commands are must-use declarations: merely constructing and
discarding one requests no work. *Command* and *effect* are therefore not
synonyms.

**EffectDescriptor** — An inert, typed description of one finite world-facing
interaction. Each Command occurrence creates a distinct effect invocation,
even when two descriptors contain equal-looking data. EffectDescriptors need
not be comparable or cloneable.

**EffectCapability / `EffectCapability<D>`** — An inert Program-issued value
declaring that its Program may issue EffectDescriptor type `D`. The capability
is required by every public Effect Command constructor but performs no I/O and
contains no Driver or runtime handle. Cloning it does not declare another
dependency. Its private Program provenance prevents it from authorizing work
in another Program.

**EffectOutcome** — The single terminal completion of an effect invocation: a
typed success, a failure carrying typed Error data, or cancellation as defined
by that effect's contract. `Command::effect(&capability, effect)` supplies the
one-shot mapper through `Message: From<EffectOutcome<Output, Error>>`, while
`Command::effect_with(&capability, effect, mapper)` supplies it explicitly. A
Command may instead explicitly discard the outcome when no terminal
application reaction is meaningful. That mode remains runtime-owned finite
work rather than "fire and forget". Aborting the entire live runtime scope under
ADR-0004 is ownership cleanup rather than an effect-contract cancellation: the
live future is dropped or cancelled and no EffectOutcome or mapped Message is
manufactured for an application that is ending.

**HttpRequest** — The first-party terminal EffectDescriptor for one raw finite
HTTP interaction. It owns method, URL text, headers, and body bytes; it is not
an executing client or a typed application endpoint.

**HttpResponse** — The complete raw output of one HttpRequest: status, version,
headers, and fully buffered body bytes. An HTTP error status is still an
HttpResponse; application status policy is distinct from transport failure.

**HttpError** — Typed explanatory data for failure to configure or transport a
first-party HttpRequest. It does not represent an HTTP status selected by the
remote endpoint.

**HttpResponsePipeline** — An inert, must-use, one-shot pure continuation
created when `HttpRequest::on_response` consumes a request. It declares ordered
response policy and decoding, then lowers through `into_command(&capability)`
using the matching EffectCapability and the Component Message's canonical
`From` conversion, or through `into_command_with(&capability, mapper)` using an
explicit conversion. Both produce the same raw HttpRequest Effect plus a
composed Message mapper. It is not a Driver, runtime Layer, or separately
traced Effect.

**HttpResponseError** — The non-exhaustive response-pipeline error algebra
distinguishing a raw configuration/transport HttpError, an explicitly selected
status-policy rejection, and JSON decoding failure. Runtime cancellation is
not a variant; it remains `EffectOutcome::Cancelled`.

**HttpStatusError** — A non-2xx HttpResponse rejected by an explicit
`require_success` policy. It retains the complete raw response, including
headers and diagnostic body.

**HttpJsonError** — Failure to decode an owned HttpResponse body as JSON. It
retains both the complete raw response and the original `serde_json::Error`.

**StdinLines** — The first-party terminal SourceDescriptor for UTF-8 lines from
process standard input. Each Item is a `String` with its terminating LF and one
immediately preceding CR removed. Empty lines are preserved; nonempty bytes at
clean EOF form one final unterminated Item before `Ended`. The descriptor is
platform-neutral, while Samara's first-party `bind_stdin` live realization is
currently Unix-only.

**StdinError / StdinErrorKind** — Typed explanatory data for terminal
`StdinLines` failure. `Read` identifies failure to read process input, and
`InvalidUtf8` identifies a line payload, including final unterminated input,
that was not valid UTF-8.
`StdinError::new` constructs the same boundary value in controlled fixtures.
Either failure produces `SourceEvent::Failed` without a later `Ended`.

**stdin live binding / `bind_stdin`** — The Unix-only first-party exact binding
of process stdin to one capability whose terminal descriptor is `StdinLines`.
The capability may be direct or place built-in Layers above that terminal
descriptor. One live runtime has at most one such binding, and first-party
bindings across runtimes share one process-wide active lease. Each realization
owns an interruptible reader thread; removal, replacement, shutdown, fault,
EOF, or failure joins it before releasing the Source. Code outside Samara must
not read process stdin concurrently. Controlled execution uses ordinary
type-wide `control_source::<StdinLines>()` behavior rather than this live
binding.

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

**SourceCapability / `SourceCapability<S>`** — An inert Program-issued value
declaring that its Program may subscribe to SourceDescriptor type `S`. It is
required by every public Source Subscription constructor and has private
Program provenance and identity. For a composed `S`, the one outer capability
also records the sealed lowered terminal descriptor requirement; applications
do not separately declare inner descriptors or Layers.

**Terminal descriptor** — An EffectDescriptor or SourceDescriptor directly
realized by a matching Driver in live execution or by terminal controlled
behavior. `TcpBytes` is a terminal SourceDescriptor.

**Composed descriptor** — An EffectDescriptor or SourceDescriptor built by one
or more Layers around another descriptor. Interpretation applies those Layers
until it reaches a terminal descriptor. `Framed<TcpBytes, D>` is a composed
SourceDescriptor whose framed event vocabulary is visible to its Subscription.

**SourcePlan** — The runtime-owned compiled mechanism produced by lowering one
desired composed SourceDescriptor during reconciliation. It contains the
terminal SourceDescriptor, its ordered profile-independent Layers,
SourceCapability identity, and Subscription message mapper. It is not another
application declaration, Subscription identity, or running Source.
Applications bind live or controlled behavior only for the terminal
descriptor.

**Subscription / `Subscription<Message>`** — A Component's declarative desire
to maintain a Source matching a SourceDescriptor under a stable,
Component-local identity through a SourceCapability, plus a message mapper from
SourceEvents to Component Messages.
`Subscription::source(&capability, id, descriptor)` obtains that mapper through
`Message: From<SourceEvent<Item, Error>>`;
`Subscription::source_with(&capability, ...)` accepts it explicitly. Declaring
a Subscription does not itself start work, and the constructor spelling does
not affect reconciliation.

**Source** — The runtime-scoped ongoing realization executing behind a
SourceDescriptor. It may contain operational resources such as tasks, sockets,
and partial framing buffers. A Source can produce zero or more SourceEvents
until it ends, fails, or is canceled.

**Source generation** — Private runtime identity for one Source realization.
Replacing a descriptor atomically withdraws the old generation. Its events or
mapped Messages that have not begun a Component transition are stale, are
discarded, and are recorded in the structural trace. Components observe a
generation only when the application deliberately carries its own domain
generation.

**SourceEvent** — One repeatable occurrence produced by a Source. A reusable
message mapper transforms each SourceEvent into a Component Message.

**Source terminal arbitration** — The accepted Phase 6 rule that exactly one
terminal condition wins while a live Source generation is active. An accepted
`SourceSink::end`, accepted `SourceSink::fail`, or active SourceDriver return
produces one terminal event; a cancellation, replacement, shutdown, or runtime
fault cutover winning first suppresses later terminal events. Sink calls after
termination return `DriverStopped`.

**Desired Subscription** — A Subscription returned from the Component's
current Model during reconciliation.

**Active Subscription** — Runtime bookkeeping for desired ongoing work,
including any currently maintained Source.

**Subscription reconciliation** — The post-transition comparison of desired
and active Subscriptions. A new stable identity starts a Source; the same
identity, SourceCapability, and equal SourceDescriptor retain it; the same
identity with a changed capability or descriptor replaces it; a removed
identity cancels it. Removal or replacement does not inherently manufacture a
SourceEvent.

When a Source is retained, reconciliation atomically installs the latest
mapper returned by `subscriptions()`. Already-created Messages are unchanged;
subsequent SourceEvents use the latest mapper. Replacement is instead a hard
Source-generation cutover.

**Message mapper** — Pure synchronous application logic that converts boundary
data into a Component Message. EffectOutcome and request-outcome mappers are
one-shot and may be represented by `FnOnce`; SourceEvent and protocol-binding
mappers are reusable and may be represented by `Fn`. Mapper object identity is
not itself part of Command or Subscription semantics. Continuation-bearing APIs
use the short method when `Message: From<BoundaryValue>` expresses one
canonical conversion and a `_with` method for an explicit call-site mapper.
These are two ways to supply the same semantic mapper, not different runtime
operations.

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

**Type-wide binding** — One live Driver or controlled behavior registration
that can satisfy every declared capability lowering to the same terminal
descriptor type.

**Exact binding** — A profile binding associated with one private capability
identity because it owns one concrete resource rather than an implementation
for every value of a descriptor type. The first-party one-shot `mpsc` receiver
bridge selects one independently bindable receiver. Unix `bind_stdin` also
selects one exact capability, but process stdin permits only one such binding
in a live runtime even when the capabilities differ.

Under ADR-0004, normal EffectDriver return maps success or failure
exactly once. A SourceDriver that returns without an accepted terminal sink
call normally ends its still-active Source exactly once. Driver panic instead
faults the runtime and produces no fabricated typed application result.

Controlled execution provides deterministic behavior for the same terminal
descriptor contracts without silently invoking live Drivers. Normal Driver and
controlled registrations are type-wide; exact resource bridges such as Tokio
`mpsc` and Unix process stdin select one SourceCapability identity.

The finite and ongoing lifecycles are deliberately similar only where their
semantics are actually similar:

```text
Command + EffectCapability + EffectDescriptor
    -> zero or more Layers
    -> terminal EffectDriver in live execution, or controlled behavior
    -> EffectOutcome exactly once on normal completion
       (whole-scope abort may produce none)
    -> one-shot message mapper -> Component Message,
       or explicitly discard outcome -> no Message

Subscription(identity + SourceCapability + SourceDescriptor + message mapper)
    -> reconciliation
    -> SourcePlan(ordered Layers + terminal descriptor + latest mapper)
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

**Decoder finalization** — The pure EOF operation required for Phase 6. Normal
ending of the underlying Source asks the Decoder to emit zero or more final
frames or one typed decode Error. Final frames precede `Ended`; finalization
failure emits `Failed` without `Ended`. Underlying failure, an earlier decoder
failure, replacement, cancellation, shutdown, and runtime fault are not decoder
EOF and do not invoke finalization.

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
`Descriptor` suffix. Unix `bind_stdin(&capability)` names the unique live
process resource. It removes CRLF or LF framing, preserves empty lines, emits a
final unterminated line before clean EOF, and reports read or UTF-8 failure as
typed Source Error data. Private readiness, cancellation, and reader-thread
failures remain runtime faults. Controlled execution scripts the same terminal
event vocabulary without opening stdin.

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

**Port** — A named, inert Program-issued logical dependency on a Protocol. A Port contains no
provider reference, channel, runtime handle, or lookup capability. It remains
distinct from the live-only `PortHandle` obtained from an assembled runtime.

**Provider** — The concrete Component selected during program assembly to
receive Protocol Messages sent through a Port.

**Requester** — The origin of a Request through a Port. A Component requester
owns a Message continuation and any domain correlation. A live host requester
awaits its Reply through `PortHandle`. In both cases the runtime owns transport
correlation.

**Port binding** — The assembly-time mapping from one exact named Port to a
provider and its private Component Message vocabulary. The provider's Message
implements `From<Protocol::Message>`, making the pure vocabulary conversion a
standard type relationship rather than a closure repeated at each binding.
Multiple named Ports of the same Protocol may still be bound independently.

**Notification** — A one-way Protocol value implementing `Notification<P>` and
sent through `Command::notify` or live `PortHandle::notify`. It has no correlated
terminal outcome for the sender. Notification delivery-failure policy beyond
accepted live admission and whole-scope cutovers remains unresolved.

**Request** — A Protocol operation/value implementing `Request<P>` with a
statically associated `Reply` type. `Command::request(port, request)` issues it
as finite correlated work using
`Message: From<RequestOutcome<Reply>>`; `Command::request_with` accepts an
explicit continuation. Its eventual typed RequestOutcome is transformed into
an ordinary requester Component Message; the requester does not await inside
`update`. A live host may instead issue the same value through
`PortHandle::request` and await `Result<Reply, RuntimeError>` directly.

**RequestInvocation** — One dynamic Request occurrence delivered to the
provider. It contains the Request value and a one-shot `ReplyTo` authority. The
runtime, not application code, associates the invocation with its eventual
outcome.

**Reply** — The successful response type statically associated with a Request.
A provider emits a value of that type using the invocation's `ReplyTo`
authority.

**Request outcome / `RequestOutcome`** — The Component-requester-visible single
terminal outcome of a Request, provisionally a Reply, a failure carrying runtime
Error data, timeout, or cancellation. The exact in-band failure and deadline
policy is not yet settled. Phase 5 implements only `RequestOutcome::Replied`; an
unanswered Request remains pending until controlled cancellation cleans up
ownership without manufacturing the deferred outcome variants. ADR-0006's live
host request returns `Reply` directly or a host-boundary `RuntimeError`; it does
not manufacture a RequestOutcome.

**`RequestError`** — The current provisional type for a Request's runtime-level
terminal error data. Domain-level negative replies remain ordinary Reply
values.

**Request continuation** — The one-shot Message mapper attached to a
Component-issued Request. The default `Command::request` form obtains it through
the requester's canonical `From<RequestOutcome<Reply>>` conversion. The explicit
`Command::request_with` form may capture domain context or assign
call-site-specific meaning. Both produce the requester's Component Message
through the same runtime lifecycle. A live host Request has a waiter rather than
a Message continuation.

**Transport correlation** — Opaque runtime bookkeeping that distinguishes one
dynamic RequestInvocation from every other invocation. Components and live host
callers neither create nor compare transport-correlation identifiers.

**Domain correlation** — Application-owned identity that explains the business
meaning of a Request or Reply. It is commonly captured by the request
continuation and returned in the resulting Component Message.

**`ReplyTo`** — The current provisional name for an opaque, one-shot authority
that permits a provider to emit the correctly typed Reply. It is inert data,
not a channel, future, or runtime handle.

**`ComponentRef`** — The current provisional name for an inert typed logical
address issued during Program assembly when direct coupling to another
Component's complete Message API is deliberate.

**`ComponentHandle`** — The current provisional name for a live
external-ingress capability available at the Samara program boundary. Unlike a
`ComponentRef`, it is a runtime capability and must not be available inside
Components. Under ADR-0004, successful `send` means accepted for
runtime-managed delivery, not transition completion; shutdown or a runtime
fault closes admission and later sends fail explicitly.

**`PortHandle`** — A cloneable live external-ingress capability for one exact
bound `Port<P>`. `LiveRuntime::port_handle` validates the Port against its built
Program. The handle admits Notifications and typed Requests through the normal
Protocol binding without exposing the provider. A host Request awaits `Reply`
directly; Drain retains it, Cancel or clean closure wakes it with `RuntimeError`,
and a runtime fault preserves that fault. Dropping an admitted waiter does not
cancel runtime-owned work. PortHandle is not inert Component wiring and has no
ControlledRuntime counterpart under ADR-0006.

**`RuntimeTask`** — The unique live structured-ownership handle returned by
`LiveRuntime::spawn`. `shutdown` consumes it and explicitly selects Drain or
Cancel. Under ADR-0007, `run_forever(&mut self)` observes owner termination
without initiating shutdown; cancelling that observation leaves the task
owning the scope so a host can select it against its own shutdown future.

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
explicitly rather than falling back to live behavior. Reaching an unhandled
terminal descriptor faults the run without invoking a live Driver or message
mapper; state and trace remain inspectable and controlled cancellation remains
available.

**Logical time / controlled time** — Runtime-controlled time that can advance
manually or automatically without wall-clock sleeping.

**Runtime semantics** — The observable scheduling, lifecycle, and tie-breaking
rules whose sameness is part of a controlled-determinism claim.

**Deterministic insertion ticket** — The stable secondary key used after
logical deadline to order equal-time controlled work. One transition's
Commands follow declaration traversal order, controlled inputs follow harness
order, initial Component work follows canonical Component identity rather than
registration order, and causally emitted work follows its cause. Tickets are
controlled reproducibility machinery, not live domain order.

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

**Admission** — The point at which live boundary input becomes
runtime-owned work. ADR-0004 gives `ComponentHandle::send` and `SourceSink` this
meaning; ADR-0006 extends the same external cutoff to PortHandle Notifications
and Requests. A PortHandle future attempts admission when first polled. A call
racing shutdown is either accepted under the selected shutdown policy or
rejected; accepted work is not silently dropped because an internal capacity
was reached.

**Unbounded internal delivery** — The accepted v0 live mechanism after
admission. It has no configurable capacity or silent overload drop and can
therefore grow memory without bound under sustained pressure. This is a
documented first-cut limitation, not a stable throughput, fairness, or
backpressure promise.

**Drain shutdown / `Shutdown::Drain`** — Structured closure that
closes external ingress, stops Sources, realizes no new Sources, and recursively
processes accepted and causally emitted finite work. It has no implicit
deadline and may wait forever for Drivers, Requests, timers, or self-sustaining
application work.

**Cancel shutdown / `Shutdown::Cancel`** — Structured closure that
closes ingress, stops application driving, cancels semantic obligations and
runtime-owned tasks, and joins or aborts all owned work without manufacturing
application outcomes merely because the scope ended. Pending live host Request
waiters are woken through the out-of-band `RuntimeError` channel rather than a
fabricated RequestOutcome.

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
| Component Message and Protocol Message | A Component Message is private transition input; a Protocol Message is the provider-neutral vocabulary crossing a Port. |
| Request and RequestInvocation | A Request is the typed operation/value; a RequestInvocation is one dynamic occurrence carrying Request and `ReplyTo`. |
| Port and provider | A Port is an inert dependency; the provider is the Component selected during assembly. |
| Notification and Request | `Notification<P>` is one-way input issued by `Command::notify` or live `PortHandle::notify`; `Request<P>` has an associated Reply and is issued by `Command::request` with a canonical continuation, `Command::request_with` with an explicit one, or live `PortHandle::request` with a direct host waiter. |
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
| Component value | Component configuration | Name the role rather than implying a second application unit. |
| `Msg` | Message / `Component::Message` | Use *Component Message* when it must be distinguished from a Protocol Message. |
| `Cmd` | Command | Use the full word in type and method names. |
| Effect value / Effect intent | EffectDescriptor | Use *effect* generically for the interaction, not as a second ambiguous type noun. |
| `EffectEvent` | EffectOutcome | An accepted EffectOutcome is terminal and occurs once; whole-scope abort may close ownership without one, while repeatable occurrences are SourceEvents. |
| Error payload types named `*Failure`, such as `TcpFailure` | `*Error`, such as `TcpError` | Error names explanatory data; failure names the semantic occurrence carrying it. |
| Result mapper | Message mapper | Name what the pure function produces. A request's one-shot mapper is its request continuation. |
| Source meaning a descriptor | SourceDescriptor | Reserve *Source* for the runtime-scoped ongoing realization. |
| `MpscSource<T>`, `MpscInput<T>` | `StreamDescriptor<T>` | Name the inert logical stream independently of its live adapter; reserve Source for its runtime realization. |
| `Incoming<P, R>` | `RequestInvocation<P, R>` | Name the dynamic Request occurrence, not merely its direction. |
| `Protocol::Inbound`, `HealthInbound` | `Protocol::Message`, `HealthProtocolMessage` | Name what the enum contains and associate concrete names with their Protocol. |
| `ActorRef`, `Addr`, `PortRef`, `RuntimeRef` | `ComponentRef`, `ComponentHandle`, `Port`, or `PortHandle` | Choose the term that distinguishes logical address, live private-Message ingress, inert Protocol dependency, or live Protocol ingress. |
| `ask` | `request` | Use `Command::request` for command-now, Message-later Component request/reply and `PortHandle::request` for the deliberately awaitable live host boundary. |
| `tell` | `notify` or `send` | Use *notify* for provider-neutral one-way Protocol interaction through `Command` or `PortHandle`, and *send* for deliberate direct delivery to a Component Message API. |
| `ReplyToken` | `ReplyTo` | `ReplyTo` is the current provisional spelling; do not canonize both. |
| Simulated execution | Controlled execution | Use *controlled* for the semantic profile; use *simulation* for the workload or product use case. |
| Mailbox loop | Runtime topology | A mailbox or event loop may be an implementation technique, not the definition of a Component. |
| Effect handler, interpreter, backend, source adapter | Driver or controlled behavior, qualified by role | Use *Driver* for a terminal live-world code boundary, *Layer* for declarative composition, and *Adapter* only as the umbrella concept. Controlled execution supplies behavior without falling through to live Drivers. |

## Open API Questions

These questions are intentionally recorded rather than answered by the Phase 2
Component-kernel freeze:

- The exact RequestOutcome variants and the deadline, cancellation,
  late-Reply, abandoned-Reply, and delegation policies beyond Phase 5's
  successful Component Reply path and ADR-0006's live-host whole-scope closure
  diagnostic.
- Notification delivery-failure semantics beyond accepted live admission and
  whole-scope Cancel/fault cutovers.
- The initial code shape of Layer abstractions. Multiple Layer kinds are
  expected, so this checkpoint does not promise one universal `Layer` trait.
- General live and controlled execution-profile binding abstractions beyond
  ADR-0008's type-wide Driver and exact Source-capability forms.
- Named capability bundles, capability identity inspection, and any blessed
  dynamic or ambient escape hatch. The current Program capability set is
  closed after assembly.
- Descriptor/message payload tracing, typed trace projections, streaming/live
  observer APIs, and durable trace storage. ADR-0004 explicitly leaves
  a public live observer outside v0.
- Bounded internal delivery, overload control, shutdown deadlines and
  escalation, exact shutdown diagnostic counts, Driver recovery, and
  restartable or shared bridges beyond ADR-0004's simple first cut.
- Exact public bridge module/type names and the exact Rust spelling of Decoder
  finalization, provided ADR-0004's accepted observable semantics are
  preserved.
