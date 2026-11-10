# Samara Vision

- Status: Draft
- Date: July 25, 2026
- Scope: Enduring product direction and observable semantics

## Headline

Samara is a framework for building asynchronous applications in Rust. Its north star is
as follows:

> Samara enables application programming on Tokio that is as side-effect-free as
> practical: state transitions are pure, effect descriptions are explicit values, and
> asynchronous execution is confined to controlled boundaries.

Useful applications necessarily interact with time, I/O, concurrency, and other systems.
Samara does not attempt to eliminate those interactions. It makes them explicit,
confines their execution, and returns their behavior to application logic as data.

The result should be application logic that is straightforward to reason about in live
execution and fully reproducible when run in a controlled test world.

## Document Role

This document defines Samara's north star. It is normative about the observable
semantics Samara aims to preserve, but it is not an API specification or a
runtime-topology decision. [`api-guidance.md`](api-guidance.md) records the
consumer-facing design rationale beneath this vision, and
[`glossary.md`](glossary.md) defines the vocabulary both use.

ADRs select concrete strategies and may narrow the scope of a release. They must
not silently weaken this vision's observable guarantees. Where an ADR refines a
guarantee stated here, the ADR is the precise statement and this document is the
intent.

## The Samara Program Boundary

A **Samara program** is a closed, declared collection of Components and the capabilities
through which they may communicate or interact with the world. `ProgramBuilder` issues
Component references, Ports, Effect capabilities, and Source capabilities during
assembly. Those inert values are threaded into immutable Component configuration; after
the Program is built, execution cannot add another dependency.

EffectDescriptors and SourceDescriptors still carry the concrete, Model-derived intent
for each occurrence. Capabilities declare which descriptor kinds the Program is allowed
to issue. A concrete execution binds their terminal descriptor requirements to live
Drivers or controlled behavior, plus a runtime configuration and a surrounding world.
Both execution profiles validate every declared requirement synchronously before
execution begins. Samara's guarantees apply within that assembled boundary.

This is a closed logical capability inventory, not field introspection or static analysis
of every behavior-dependent message edge. Rust cannot prevent deliberately
non-conforming code from hiding a capability issued by another Program. Samara rejects
such a value when it becomes observable, before it reaches a Driver or controlled
behavior; it does not treat that fault as legitimate dynamic dependency discovery.

A Samara program may be embedded in a larger Tokio process. Code outside the program
boundary is part of the surrounding world and interacts with the program through
explicit boundary adapters or ingress APIs. Samara does not claim control over arbitrary
code elsewhere in the process.

That surrounding host may await a typed result from a live ingress API, but the result
does not grant access to Component state. Any host decision that should change Samara
application state must re-enter through the program's typed Message or Port boundary.

This boundary makes incremental adoption possible without diluting the meaning of a
conforming Samara program.

## Component: Samara's Native Application Unit

**Component** is the name for Samara's topology-neutral unit of state and behavior. When
we say "topology-neutral" we mean that a Component's semantics do not depend on whether
it is implemented as a single loop, one loop per Component, or another scheduler.

A Component:

- Has a stable logical identity.
- Owns its Model and is the sole authority allowed to change it.
- Accepts typed Component Messages.
- Applies one pure transition at a time.
- Emits typed Commands describing finite work.
- Declares Subscriptions describing desired ongoing Sources.
- Interacts with other Components only through typed Protocols and Messages.

Conceptually:

```text
update(&self, Model, Message) -> (Model, Vec<Command<Message>>)
subscriptions(&self, &Model) -> Vec<Subscription<Message>>
```

These signatures describe semantics, not a final Rust API. Exclusively owned, in-place
mutation may be used as an implementation technique when it is observationally
equivalent to producing a new model value.

The Rust type and `impl Component` form the Component implementation. A particular
immutable value of that type is its Component configuration and may contain logical
wiring such as Ports, EffectCapabilities, SourceCapabilities, and declarative descriptor
configuration. Receiving `&self` signals observational
immutability; because Rust permits interior mutability, conformance still requires that
behaviorally relevant mutable state belong in the Model.

A Component does not imply a mailbox, Tokio task, thread, or event loop. Those are
runtime decisions.

### Ownership and Atomicity

- Component state is private.
- A Component never has two overlapping transitions.
- Components cannot directly read, mutate, or invoke one another.
- Cross-Component communication is explicit Message delivery requested by a Command.
- State requiring joint atomicity belongs in one Component or behind an explicit
  coordinating Component.

Global serialization must not be used as a substitute for deliberate state ownership.
Independent Components may be executed independently when doing so preserves the
semantics in this document.

## Developer Experience North Stars

The following are qualitative product checks rather than runtime conformance
requirements. They describe the experience Samara should create for the people writing
and reviewing Components.

- Samara code is *readable*, but not unnecessarily *terse*.
- Samara code is *ergonomic*, but not *magic*.
- Samara Components are *larger* than "normal" Rust, but they pay for themselves.

### Readable, Not Unnecessarily Terse

Samara code should make state transitions, EffectDescriptors, Subscriptions, and Component
interactions easy to follow. Concision is welcome when it improves clarity, but
minimizing lines or tokens is not a goal. Explicit intermediate types and steps are
preferable when they preserve meaning at the point of use.

### Ergonomic, Not Magic

The common path should require little ceremony, but convenience must not hide the
semantic machinery that makes Samara trustworthy. A user should be able to trace where a
message came from, what state changed, which command or subscription requested an
effect, and how its outcome returned. Generated code, macros, and defaults are
acceptable when they preserve that explanatory path.

### Components Are Substantial, and Pay for Themselves

A Component is a larger architectural unit than an ordinary Rust type. Not every struct,
function, or asynchronous operation should become a Component. The boundary is justified
when it buys meaningful state ownership, isolation, testability, explicit protocols, or
reuse across live and controlled execution.

The ceremony of defining a Component should remain proportional to those benefits. If a
Component boundary adds indirection without making behavior clearer, safer, or easier to
test, it is not paying for itself.

These north stars should be checked through examples, onboarding exercises, code review,
and user feedback rather than reduced to mechanical conformance tests.

## Practical Purity

The purity Samara seeks is observational rather than syntactic.

For a conforming Component, the observable result of a transition depends only on its
fixed Component implementation and configuration, Model, and input Message. A transition
receives no ambient capability for I/O, time, randomness, locking, spawning, or runtime
access.

Samara should make the conforming path the natural path:

- Runtime and Driver handles are not available to transitions.
- Component state is not exposed through mutable runtime handles.
- All application-observable interaction with the world crosses a Program-declared
  capability and its Command or Subscription boundary.
- Outcomes that can affect application decisions return as Component Messages.

Rust cannot prevent deliberately non-conforming code from reading globals, performing
direct I/O, or spawning work. Purity therefore depends on both runtime correctness and
user conformance. Samara must state that trust boundary plainly and provide testing
practices that help authors validate it.

A future release may introduce a deliberately blessed escape hatch. Its design and the
guarantees it forfeits are deferred.

## Commands and Effects

A `Command<Message>` is an inert typed description of finite work. For a world-facing
interaction, it composes a Program-issued `EffectCapability<D>`, an explicit descriptor
`D`, and a deterministic one-shot message mapper. The capability is inert authorization
to describe work, not a handle that performs I/O.

The EffectDescriptor must remain separately identifiable and interceptable by the
runtime. A Command must not hide world interaction inside an arbitrary async closure.

Pure synchronous closures or function pointers may transform typed EffectOutcomes into
Component Messages. For example:

```text
Command::effect_with(
    &self.socket_read,
    SocketRead { socket },
    bytes -> SocketMessage::Frames(decode(bytes)),
)
```

In this example, `SocketRead` is the interceptable EffectDescriptor. Live execution uses
its terminal EffectDriver; controlled execution supplies an EffectOutcome without
invoking that live Driver. The message mapper is application logic and must remain pure
and deterministic.

Closure object identity is not part of Command semantics. Where Commands carry pure
code, conformance compares the EffectDescriptor and the observable Messages produced
when equivalent controlled outcomes are supplied. A future opt-in tracing facility may
accept a stable mapper descriptor for richer diagnosis; the v0 structural trace imposes
no such author requirement.

Whenever an EffectOutcome can influence application behavior, its success or
unsuccessful reason must be representable as a Component Message. EffectFailure
distinguishes typed errors from explicit cancellation, including deadline
cancellation (ADR-0020). Runtime
diagnostics that cannot influence application decisions may remain out of band.

## Subscriptions

Commands describe finite work. A `Subscription<Message>` declaratively describes a
Component's desire to maintain ongoing event production. It combines a stable
Component-local identity, a comparable SourceDescriptor, and a reusable message mapper
from SourceEvents to Component Messages.

Subscriptions are derived purely from current Component state. Deriving a subscription
does not start work; it describes the work the Component currently wants the runtime to
maintain. After each committed transition the runtime reconciles that desire against
the Sources it owns: it starts what is newly wanted, retains what is unchanged, replaces
what changed, and cancels what is no longer wanted, so that stale work from a replaced
Source never enters a transition. A Source failure that affects application behavior
produces an explicit Component Message. ADR-0003 and the glossary state the exact
reconciliation and generation rules.

A Source is the runtime-scoped ongoing realization behind the SourceDescriptor and may
produce zero or more SourceEvents. A live SourceDriver may own world-facing operational
resources such as socket handles and read tasks. A declarative Layer may own deterministic
mechanism state such as an incomplete framing buffer. Subscription desire remains
explicit in Component state. Protocol and application policy may live in Components or
explicit Layers, but never in core runtime mechanism. Automatic restart and retry
behavior are intentionally not defined here.

## Structured Concurrency

Samara owns the lifetime of the asynchronous work it authorizes.

- Every command and subscription executes within a runtime-owned scope.
- No detached or untracked task is permitted behind the Samara boundary.
- Completion, failure, and cancellation are observable through the applicable semantic
  or diagnostic channel.
- Drivers and Sources may use internal tasks, but those tasks remain collectively owned
  and cancellable through their runtime scope.
- Controlled execution can account for pending work and distinguish immediately runnable
  work from work awaiting logical time or controlled input.

Shutdown is part of ownership: a host may drain accepted work to completion or cancel
it, and neither manufactures application results merely because the runtime is ending.
ADR-0004 and ADR-0010 define the two cutovers.


## Execution Profiles

Samara has two first-class execution profiles:

> Live and controlled execution run the same program; they differ only in
> runtime decisions about scheduling, time, effects, subscriptions, and the
> surrounding world.

Component implementations and configurations, Models, Messages, Commands,
EffectDescriptors, SourceDescriptors, Subscriptions, pure message mappers, Layers,
codecs, and Protocol logic remain the same across profiles. Runtime decisions and
terminal world bindings differ at the narrowest genuinely impure boundary.

### Live Execution

Live execution uses real Tokio scheduling, clocks, I/O, and external systems.
Independent external operations may complete in different orders across runs. Samara
does not claim that live execution is reproducible.

Live execution does guarantee the shared Component semantics:

- Transitions for one Component do not overlap.
- For a fixed Component implementation and configuration, a given ordered
  `(Model, Message)` transition is deterministic for conforming code.
- Causal relationships established by the program are preserved.
- Explicitly sequenced work preserves the sequence promised by its contract.
- Independent events have no implicit program-wide order.

### Controlled Execution

Controlled execution is a test and simulation environment that owns the world visible to
the Samara program.

Every terminal EffectDescriptor and SourceDescriptor used by the program must have
controlled behavior. Time, randomness, identifiers, external inputs, EffectOutcomes, and
SourceEvents must be controlled or seeded. The controlled builder validates the complete
closed capability declaration set before creating a run; missing behavior never silently
falls back to a live Driver or the live world. 
For a conforming program using conforming Components, Layers, and controlled bindings,
identical initial state, Component configuration, controlled inputs, seeds, and runtime
semantics must produce the same program-wide:

- Component transition trace.
- Command and subscription trace.
- Causal relationships.
- Final state across all Components.
- Runtime-controlled scheduled future work.

This guarantee is program-wide, not merely per-Component final-state equality. Two
executions that converge to the same final models after producing different structural
command traces are not equivalent controlled executions. Same-typed descriptor payloads
are compared in direct typed-intent tests rather than copied into the generic v0 trace.

### Controlled Time

Controlled execution uses logical time and supports both:

- **Manual advancement**, where a test advances by a duration or to a specific logical
  instant.
- **Automatic advancement**, where an executor with no immediately runnable work
  advances to the next scheduled event until a condition, deadline, or stopping point is
  reached.

Events at the same logical time follow a deterministic insertion order defined by
ADR-0003. That rule is reproducibility machinery, not a live or domain ordering
guarantee. Testing tools may explore alternative schedules only as explicitly different
runtime semantics.

## Ordering Without a Mandated Topology

This vision does not require a global total execution order.

- Each Component has a serialized transition history.
- Causally related events preserve their required ordering.
- Independent Components may make progress concurrently.
- Independent simultaneous events are semantically unordered.
- Controlled execution selects a deterministic schedule to produce a reproducible
  program-wide trace.

At minimum, causal ordering means:

- An effect outcome follows the command that requested the effect.
- Message delivery follows the send event that requested delivery.
- A reply follows its request.
- Explicitly sequenced or FIFO work preserves only the ordering promised by its
  contract.

A single loop, one loop per Component, or another scheduler may conform if it preserves
these observable semantics. The vision evaluates an implementation by what programs can
observe, not by its internal topology.

## Observability

Samara distinguishes application observability from runtime diagnostics.

### In-Band Semantic Results

Components observe the world only through Messages. Every behaviorally relevant result
of a declared Command or Subscription must be representable as a typed Component
Message.

### Out-of-Band Semantic Trace

Controlled execution always collects an in-memory structural semantic trace suitable for
tests and diagnosis. Each record has identity, logical time, causation, and an event. The events represent, at minimum:

- Component identity.
- Message receipt and transition commitment.
- Command kinds, concrete descriptor types, targets, and desired Subscriptions produced by
  a transition.
- EffectOutcome and Source lifecycle results, including stale-generation drops.
- Logical time and immediate causation.

The v0 generic trace is structural rather than domain-payload-complete. Descriptor and
Message payloads remain directly testable through typed Component tests; copying them into
the program-wide trace is optional future work. Reading the trace has no in-band path back
into Components. A future live observer may perturb timing and therefore the order of
independent events, but it must not weaken any promised semantics. Structured tracing does
not imply durable event sourcing, although the architecture should preserve a path for
feeding captured live-world inputs and outcomes into controlled tests.

The initial v0 live runtime is not required to expose that future observer. Internal test
instrumentation is not a public semantic surface.

Incidental runtime mechanics such as thread placement, task identifiers, and queue depth
remain diagnostics unless deliberately promoted into a public semantic contract.

## Correctness Is a Shared Contract

Samara's vision depends on both runtime correctness and user conformance.

### Runtime Guarantees

A conforming runtime is responsible for:

- Component state isolation and non-overlapping transitions.
- Message-mediated state change.
- Interceptable Commands and reconciled Subscriptions.
- Structured ownership of asynchronous work.
- The documented live and controlled execution semantics.
- Non-influential structured semantic tracing.

### Component-Author Obligations

A conforming Component author is responsible for:

- Pure and deterministic transitions.
- Pure and deterministic message-mapping closures.
- Keeping all application state under explicit Component ownership.
- Avoiding direct access to ambient I/O, time, randomness, shared mutable state, or task
  spawning from pure paths.

### Layer, Driver, and Controlled-Behavior Obligations

Authors of conforming Layers, Drivers, and controlled behavior are responsible for the
applicable obligations below:

- Keeping world interaction behind declared EffectDescriptor or SourceDescriptor
  contracts.
- Keeping Layers declarative, deterministic, and free of ambient I/O.
- Preserving structured ownership of Driver and Source work.
- Returning behaviorally relevant EffectOutcomes and SourceEvents through message
  mappers.
- Providing deterministic controlled behavior for every supported terminal descriptor.
- Introducing no hidden live Driver or live dependency into controlled execution.

The determinism claim is therefore conditional:

> A conforming Samara program, using conforming Layers and controlled bindings in a
> closed controlled world, executes deterministically.

## Testing as a First-Class Practice

Samara treats testing as a first-class design practice. Its programming model must keep
open direct, deterministic testing of Components and whole programs without requiring
the live world.

The architecture should support:

- Testing transitions directly as fixed Component configuration plus
  `Model + Message -> Model + Commands`.
- Inspecting Commands, EffectDescriptors, and Subscriptions without executing live
  Drivers.
- Running programs with controlled time and scripted world behavior.
- Asserting against structured semantic traces.
- Injecting EffectOutcome failure, Source failure, and cancellation.
- Reusable conformance suites for runtimes, Components, Layers, Drivers, and controlled
  behavior.
- Future replay and schedule-exploration tooling.

This is a commitment to preserve testability, not a promise that every testing tool
ships in the first release.

## Tokio Interoperability and Adoption

Samara meets Tokio through explicit, first-class interoperability boundaries. Common
Tokio work and event sources should be expressible as EffectDescriptors and
SourceDescriptors without exposing runtime capabilities to Components.

Samara intends to provide a small first-party set of Layers and Drivers. The exact
catalog belongs to the roadmap; it is not promised here. The canonical initial proof is
a Tokio `mpsc` receiver translated into Component Messages through a first-party Source
boundary.

Conceptually:

```text
Component:
    Subscription::source_with(
        &packet_source,
        packet_input,
        PacketMessage::Received,
    )

Live profile:
    bind packet_source to a first-party Tokio mpsc Source

Controlled profile:
    control packet_source and supply scripted SourceEvents
```

This is schematic rather than a commitment to the concrete descriptor or binding API.

The canonical shallow onboarding path is:

1. Start inside an ordinary `#[tokio::main]` application.
2. Define one Component with a Model, Messages, and a pure transition.
3. Feed it from an existing `mpsc` receiver through a first-party Source boundary.
4. Observe its behavior without implementing runtime infrastructure.
5. Reuse the same Component in a controlled test with scripted input.

Advanced schedulers, Layers, Drivers, Subscriptions, and testing capabilities should be
introduced through progressive disclosure. A user must not need to learn the entire
architecture before receiving value.

## Executable Vision Requirements

The vision is enforceable only when its observable semantics emerge from the
implementation, so its requirements are stated as topology-neutral scenarios, V1
through V12, that a conforming runtime and representative programs must demonstrate.
Their definitions and the evidence for each live in the
[acceptance matrix](testing/v0-acceptance-matrix.md).

## Not Promised Here

This vision deliberately does not promise:

- A particular runtime topology.
- A global total execution order.
- Deterministic ordering of independent events during live execution.
- Proof that arbitrary Rust Components, closures, Layers, Drivers, or controlled-world
  code conforms.
- Dynamically adding Components, Ports, Effect capabilities, or Source capabilities to a
  running Program.
- Transparent distribution of Components across processes.
- A mature bounded-pressure, overload, retry, or operational shutdown policy beyond the
  simple first live contract accepted in ADR-0004.
- Durable persistence, event sourcing, or a durable replay system.
- A comprehensive Tokio Layer and Driver catalog in the initial product.
- A complete Component, Layer, Driver, controlled-world, and runtime conformance
  toolkit in the initial product.
- Maximum throughput at the expense of the semantic guarantees above.

These are not permanent prohibitions. They are boundaries that keep this vision clear
and leave later choices to focused ADRs and roadmaps.

Open questions and deferred decisions are tracked in
[`notes/framework-follow-ups.md`](notes/framework-follow-ups.md), not here.
