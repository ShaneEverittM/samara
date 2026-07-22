# Samara Vision

- Status: Draft
- Date: July 21, 2026
- Scope: Enduring product direction and observable semantics

## Headline

Samara is a framework for building asynchronous applications in Rust. Its north star is
as follows:

> Samara enables application programming on Tokio that is as side-effect-free as
> practical: state transitions are pure, effects are explicit values, and asynchronous
> execution is confined to controlled boundaries.

Useful applications necessarily interact with time, I/O, concurrency, and other systems.
Samara does not attempt to eliminate those interactions. It makes them explicit,
confines their execution, and returns their behavior to application logic as data.

The result should be application logic that is straightforward to reason about in live
execution and fully reproducible when run in a controlled test world.

## Document Role

This document defines Samara's north star. It is normative about the observable
semantics Samara aims to preserve, but it is not an API specification or a
runtime-topology decision. [`api-guidance.md`](api-guidance.md) records the
consumer-facing design rationale that should guide API work beneath this vision.
[`glossary.md`](glossary.md) defines the canonical vocabulary used by both documents.

ADRs may select concrete implementation strategies and may narrow the scope of an
initial release. They must not silently weaken this vision's observable guarantees.
For example: ADR-0002 superseded ADR-0001's single-mailbox and global-order mandate:
runtime topology is an implementation choice, while per-Component serialization,
causality, and controlled determinism are observable contracts.

## The Samara Program Boundary

A **Samara program** is a declared collection of Components and their protocol, effect,
and subscription contracts. A concrete execution binds those contracts to adapters, a
runtime configuration, and a surrounding world. Samara's guarantees apply within that
assembled boundary.

A Samara program may be embedded in a larger Tokio process. Code outside the program
boundary is part of the surrounding world and interacts with the program through
explicit adapters. Samara does not claim control over arbitrary code elsewhere in the
process.

This boundary makes incremental adoption possible without diluting the meaning of a
conforming Samara program.

## Component: Samara's Native Application Unit

**Component** is the name for Samara's topology-neutral unit of state and behavior. When
we say "topology-neutral" we mean that a Component's semantics do not depend on whether
it is implemented as a single loop, one loop per Component, or another scheduler.

A Component:

- Has a stable logical identity.
- Owns its model and is the sole authority allowed to change it.
- Accepts typed messages.
- Applies one pure transition at a time.
- Emits typed commands describing finite work.
- Declares subscriptions describing ongoing message sources.
- Interacts with other Components only through typed protocols and messages.

Conceptually:

```text
update(Model, Msg) -> (Model, Vec<Cmd>)
subscriptions(&Model) -> Vec<Subscription<Msg>>
```

These signatures describe semantics, not a final Rust API. Exclusively owned, in-place
mutation may be used as an implementation technique when it is observationally
equivalent to producing a new model value.

A Component does not imply a mailbox, Tokio task, thread, or event loop. Those are
runtime decisions.

### Ownership and Atomicity

- Component state is private.
- A Component never has two overlapping transitions.
- Components cannot directly read, mutate, or invoke one another.
- Cross-Component communication is explicit message delivery and therefore an effect
  intent.
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

Samara code should make state transitions, effect intent, subscriptions, and Component
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
model and input message. A transition receives no ambient capability for I/O, time,
randomness, locking, spawning, or runtime access.

Samara should make the conforming path the natural path:

- Runtime and adapter handles are not available to transitions.
- Component state is not exposed through mutable runtime handles.
- All application-observable interaction with the world crosses declared command or
  subscription boundaries.
- Outcomes that can affect application decisions return as messages.

Rust cannot prevent deliberately non-conforming code from reading globals, performing
direct I/O, or spawning work. Purity therefore depends on both runtime correctness and
user conformance. Samara must state that trust boundary plainly and provide testing
practices that help authors validate it.

A future release may introduce a deliberately blessed escape hatch. Its design and the
guarantees it forfeits are deferred.

## Commands and Effects

A `Cmd` is a typed composition of explicit effect intent and deterministic result
transformation.

The actual effect must remain separately identifiable and interceptable by the runtime.
A command must not hide the world interaction inside an arbitrary async closure.

Pure synchronous closures or function pointers may be used to transform typed effect
results into messages. For example:

```text
perform(
    SocketRead { socket },
    bytes -> Msg::Frames(decode(bytes)),
)
```

In this example, `SocketRead` is the interceptable effect. The result mapping is
application logic and must remain pure and deterministic.

Closure object identity is not part of command semantics. Where commands carry pure
code, conformance compares the explicit effect intent and the observable messages
produced when equivalent controlled results are supplied. A runtime may additionally
require a stable mapper descriptor for tracing or diagnosis.

Whenever an effect outcome can influence application behavior, its success, failure,
timeout, or cancellation must be representable as a message. Runtime diagnostics that
cannot influence application decisions may remain out of band.

## Subscriptions

Commands describe finite work. A `Subscription<Msg>` declaratively describes an ongoing
external source of messages.

Subscriptions are derived purely from current Component state. Deriving a subscription
does not start work; it describes the work the Component currently wants the runtime to
maintain.

After a committed transition, the runtime reconciles the desired subscriptions with the
active subscriptions:

- A newly desired subscription is started.
- An unchanged subscription, identified by a stable logical key, remains active without
  restarting.
- A removed subscription is canceled.
- A subscription whose resource configuration changes is replaced or reconfigured
  according to its contract.
- A failed subscription produces an explicit message.

The runtime or adapter may own operational resource state such as socket handles, read
tasks, and incomplete frame buffers. Subscription desire remains explicit in Component
state. Protocol and application policy may live in Components or explicit
adapter/protocol layers, but never in core runtime effects. Automatic restart and retry
behavior are intentionally not defined here.

## Structured Concurrency

Samara owns the lifetime of the asynchronous work it authorizes.

- Every command and subscription executes within a runtime-owned scope.
- No detached or untracked task is permitted behind the Samara boundary.
- Completion, failure, and cancellation are observable through the applicable semantic
  or diagnostic channel.
- Adapters may use internal tasks, but those tasks remain collectively owned and
  cancellable through the adapter's scope.
- Controlled execution can account for pending work and distinguish immediately runnable
  work from work awaiting logical time or controlled input.

The exact drain-versus-cancel policy for shutdown is deferred, but ownership is not
optional.

## Execution Profiles

Samara has two first-class execution profiles:

> Live and controlled execution run the same program; they differ only in
> runtime decisions about scheduling, time, effects, subscriptions, and the
> surrounding world.

Components, commands, subscriptions, pure result mappings, codecs, protocol logic, and
logical adapter contracts remain the same across profiles. Runtime bindings and
world-facing capability interpreters differ at the narrowest genuinely impure boundary.

### Live Execution

Live execution uses real Tokio scheduling, clocks, I/O, and external systems.
Independent external operations may complete in different orders across runs. Samara
does not claim that live execution is reproducible.

Live execution does guarantee the shared Component semantics:

- Transitions for one Component do not overlap.
- A given ordered `(Model, Msg)` transition is deterministic for conforming code.
- Causal relationships established by the program are preserved.
- Explicitly sequenced work preserves the sequence promised by its contract.
- Independent events have no implicit program-wide order.

### Controlled Execution

Controlled execution is a test and simulation environment that owns the world visible to
the Samara program.

Every effect and subscription used by the program must have a controlled interpreter.
Time, randomness, identifiers, external inputs, and effect outcomes must be controlled
or seeded. Missing controlled behavior fails explicitly; it must never silently fall
back to the live world.

For a conforming program and conforming adapters, identical initial state,
configuration, controlled inputs, seeds, and runtime semantics must produce the same
program-wide:

- Component transition trace.
- Command and subscription trace.
- Causal relationships.
- Final state across all Components.
- Runtime-controlled scheduled future work.

This guarantee is program-wide, not merely per-Component final-state equality. Two
executions that converge to the same final models after emitting different commands are
not equivalent controlled executions.

### Controlled Time

Controlled execution uses logical time and supports both:

- **Manual advancement**, where a test advances by a duration or to a specific logical
  instant.
- **Automatic advancement**, where an executor with no immediately runnable work
  advances to the next scheduled event until a condition, deadline, or stopping point is
  reached.

Events at the same logical time use a stable tie-break so a controlled run is
reproducible. The exact tie-break is a runtime-semantics decision, not a domain ordering
guarantee. Alternative valid schedules may also be explored by testing tools to expose
order-sensitive application logic.

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

### In-Band Semantic Outcomes

Components observe the world only through messages. Every behaviorally relevant outcome
of a declared command or subscription must be representable as a typed message.

### Out-of-Band Semantic Trace

The runtime exposes a structured semantic trace suitable for tests and diagnosis. It
should represent, at minimum:

- Component identity.
- Message receipt and transition commitment.
- Commands and desired subscriptions produced by a transition.
- Effect and subscription lifecycle outcomes.
- Logical or live timestamps.
- Causation relationships.

Trace observation has no in-band path back into Components, and the runtime must not
branch on whether an observer is present. In controlled execution, enabling or disabling
an observer must not change semantic behavior. In live execution, instrumentation may
perturb timing and therefore the order of independent events, but it must not weaken any
promised semantics. Structured tracing does not imply durable event sourcing, although
the architecture should preserve a path for feeding captured live-world inputs and
outcomes into controlled tests.

Incidental runtime mechanics such as thread placement, task identifiers, and queue depth
remain diagnostics unless deliberately promoted into a public semantic contract.

## Correctness Is a Shared Contract

Samara's vision depends on both runtime correctness and user conformance.

### Runtime Guarantees

A conforming runtime is responsible for:

- Component state isolation and non-overlapping transitions.
- Message-mediated state change.
- Interceptable commands and subscriptions.
- Structured ownership of asynchronous work.
- The documented live and controlled execution semantics.
- Non-influential structured semantic tracing.

### Component-Author Obligations

A conforming Component author is responsible for:

- Pure and deterministic transitions.
- Pure and deterministic result-mapping closures.
- Keeping all application state under explicit Component ownership.
- Avoiding direct access to ambient I/O, time, randomness, shared mutable state, or task
  spawning from pure paths.

### Adapter-Author Obligations

A conforming adapter author is responsible for:

- Keeping world interaction behind declared effect or subscription contracts.
- Preserving structured ownership of internal work.
- Returning behaviorally relevant outcomes through messages.
- Providing deterministic controlled behavior when claiming controlled support.
- Introducing no hidden live dependency into controlled execution.

The determinism claim is therefore conditional:

> A conforming Samara program, using conforming adapters in a closed controlled
> world, executes deterministically.

## Testing as a First-Class Practice

Samara treats testing as a first-class design practice. Its programming model must keep
open direct, deterministic testing of Components and whole programs without requiring
the live world.

The architecture should support:

- Testing transitions directly as `Model + Msg -> Model + Cmds`.
- Inspecting commands and subscriptions without executing live effects.
- Running programs with controlled time and scripted world behavior.
- Asserting against structured semantic traces.
- Injecting effect failure, subscription failure, and cancellation.
- Reusable conformance suites for runtimes, Components, and adapters.
- Future replay and schedule-exploration tooling.

This is a commitment to preserve testability, not a promise that every testing tool
ships in the first release.

## Tokio Interoperability and Adoption

Samara meets Tokio through explicit, first-class interoperability boundaries. Common
Tokio work and event sources should be adaptable into commands and subscriptions without
exposing runtime capabilities to Components.

Samara intends to provide a small first-party interoperability layer. The exact adapter
catalog belongs to the roadmap; it is not promised here. The canonical initial proof is
an inbound Tokio `mpsc` receiver translated into Component messages.

Conceptually:

```text
Component:
    subscribe(Inbound<Packet>) -> Msg::PacketReceived

Live world:
    bind Inbound<Packet> to TokioMpsc(receiver)

Controlled world:
    bind Inbound<Packet> to ScriptedInput(events)
```

The canonical shallow onboarding path is:

1. Start inside an ordinary `#[tokio::main]` application.
2. Define one Component with a model, messages, and a pure transition.
3. Feed it from an existing `mpsc` receiver through a first-party adapter.
4. Observe its behavior without implementing runtime infrastructure.
5. Reuse the same Component in a controlled test with scripted input.

Advanced schedulers, adapters, subscriptions, and testing capabilities should be
introduced through progressive disclosure. A user must not need to learn the entire
architecture before receiving value.

## Executable Vision Requirements

The vision is enforceable only when its observable semantics emerge from the
implementation. Conformance requirements must therefore be topology-neutral and
executable.

The final APIs and test harness are not specified here, but a conforming runtime and
representative conforming programs must be able to demonstrate the following scenarios.

### V1. Repeatable Component Transition

Given equivalent initial models and the same message, repeated execution of a conforming
transition produces equivalent next models and commands. The runtime provides no ambient
world capability to the transition.

For commands containing pure code, equivalence compares explicit effect intent and the
messages produced from equivalent controlled outcomes, not closure object identity.

### V2. Isolated State Ownership

When messages target the same Component concurrently, its transitions never overlap. No
external runtime handle can mutate its model. Interaction with a second Component occurs
only through declared messages and protocols.

### V3. Interceptable Effect

Given a command containing a typed effect intent and a pure result mapper, a controlled
interpreter can observe the intent, provide the result without performing the live
effect, and cause the mapped message to enter the target Component.

### V4. Declarative Subscription Lifecycle

Given a model-derived subscription with a stable identity:

- It starts when first desired.
- It remains active across transitions while unchanged.
- It is canceled when no longer desired.
- A same-identity subscription with changed resource configuration is replaced or
  reconfigured according to its contract.
- Its controlled events enter the Component as messages.
- Its failure enters the Component as a message.

No automatic restart behavior is asserted.

### V5. Live and Controlled Program Parity

The same Component program can be assembled once with live world bindings and once with
controlled world bindings without changing Components, commands, subscriptions, codecs,
protocol logic, or logical adapter contracts. Only runtime bindings and world-facing
capability interpreters differ.

### V6. Program-Wide Controlled Determinism

Given the same initial program, controlled world, logical-time inputs, seeds, and
runtime semantics, two executions produce equivalent semantic traces and equivalent
final state across all Components.

The trace comparison includes transitions, commands, subscriptions, outcomes, causation,
and logical time—not only final model values.

### V7. Controlled Time Progression

A controlled program can be driven once through explicit time advancement and once
through automatic advancement. Each style produces the deterministic trace defined by
its input schedule without sleeping in wall-clock time.

### V8. Causal Semantics Without Global Order

A multi-Component scenario demonstrates preserved causal relationships and
non-overlapping per-Component transitions without assuming an order between independent
live events or assuming a particular runtime topology.

### V9. Structured Work Ownership

At every observation point, the runtime can account for work created by commands and
subscriptions. Ending the owning scope leaves no detached Samara work. Failure and
cancellation are visible through the appropriate semantic or diagnostic channel.

### V10. Non-Influential Semantic Trace

Running a controlled scenario with and without a trace observer produces the same
application behavior. In live execution, the observer has no semantic feedback path even
though instrumentation may perturb the timing of independent events. The observed trace
contains enough structured information to explain transitions, effects, subscriptions,
time, and causation.

### V11. Shallow Tokio Onboarding

A compiling example embeds one Component in a normal Tokio application, adapts an `mpsc`
receiver into messages using first-party interop, and reuses the same Component in a
controlled test without custom runtime or adapter implementation.

## Not Promised Here

This vision deliberately does not promise:

- A particular runtime topology.
- A global total execution order.
- Deterministic ordering of independent events during live execution.
- Proof that arbitrary Rust Components, closures, or adapters are pure.
- Transparent distribution of Components across processes.
- A prescribed backpressure, delivery, retry, or shutdown policy beyond observability
  and structured ownership.
- Durable persistence, event sourcing, or a durable replay system.
- A comprehensive Tokio adapter catalog in the initial product.
- A complete Component, adapter, and runtime conformance toolkit in the initial product.
- Maximum throughput at the expense of the semantic guarantees above.

These are not permanent prohibitions. They are boundaries that keep this vision clear
and leave later choices to focused ADRs and roadmaps.

## Deferred Decisions

The following questions remain intentionally open:

- Whether to provide a blessed escape hatch and how it advertises weakened guarantees.
- The concrete Rust API.
- The controlled scheduler's exact equal-time tie-break.
- Subscription restart and retry semantics.
- Delivery, backpressure, overload, and coalescing policies.
- Shutdown drain-versus-cancel semantics.
- The initial first-party adapter catalog beyond the canonical `mpsc` bridge.
- The semantic trace representation, versioning, storage, and replay tooling.
- The shape and release timing of reusable conformance harnesses.
