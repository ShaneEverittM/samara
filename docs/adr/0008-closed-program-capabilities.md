# ADR 0008: Closed Program Capabilities

- Status: Accepted
- Date: July 25, 2026
- Decision owners: Samara maintainers
- Extends: [ADR 0003](0003-controlled-execution-semantics.md) and
  [ADR 0004](0004-initial-live-runtime-semantics.md)
- Supersedes: ADR-0003's open dependency-graph qualification and ADR-0004's
  allowance for a legitimately declared Effect or Source binding to be
  discovered missing only after execution starts

## Context

Samara already requires Program-issued `ComponentRef<C>` and `Port<P>` values
for Component communication. Effect Commands and Subscriptions are different:
any code with an `EffectDescriptor` or `SourceDescriptor` can currently create
one, even when Program assembly never declared that boundary.

Consequently, live and controlled builders can validate only work visible in a
Component's initial projection. A branch reached by a later Message can reveal
a missing terminal binding after the runtime has started. This weakens the
assembly boundary and makes a wiring error look like an execution failure.

Rust cannot discover every branch of arbitrary `update` code. A separate
dependency manifest could make requirements visible, but it could drift from
the Commands and Subscriptions that actually issue work. Static analysis,
typelists, or macro-owned Component bodies would add substantial magic and
restrict ordinary Rust.

Bevy's `SystemParam` pattern provides the useful principle: the declared
dependency should also be the only supported means of using that dependency.
Samara can apply that principle with ordinary Program-issued values.

## Decision Drivers

- Make every conforming Component dependency knowable before either execution
  profile starts.
- Make the dependency declaration and its means of use one value, so a separate
  manifest cannot drift.
- Preserve arbitrary ordinary Rust inside pure Component transitions.
- Preserve immutable Component configuration and the absence of an ambient
  runtime context in `update`.
- Keep composed Source Layers profile-independent and bind only terminal
  boundaries.
- Support exact resource adapters such as one-shot Tokio `mpsc` without
  pretending that a descriptor type alone identifies one resource.
- Reject accidental cross-Program wiring without making private runtime
  identity an application ordering concept.

## Decision

### Program assembly is closed

`ProgramBuilder` is the only public issuer of logical runtime capabilities.
Before registering a Component that uses a world boundary, assembly obtains an
`EffectCapability<D>` or `SourceCapability<S>` and threads it into that
Component's immutable configuration. `ComponentRef<C>` and `Port<P>` remain the
corresponding Program-issued values for concrete and provider-neutral Component
connections.

After `ProgramBuilder::build` consumes the builder, the Program's Component,
Port, Effect, and Source capability declarations are closed. Execution may
create any number of occurrences and may choose descriptor payloads from Model
state, but it cannot add a new dependency kind or exact resource capability.

The intended shape is:

```rust,ignore
let mut program = Program::builder();
let http = program.effect::<HttpRequest>();
let input = program.source::<StreamDescriptor<Input>>();

let component = program.component(
    ComponentId::new("worker"),
    Worker { http, input },
);
let program = program.build()?;
```

Capability creation is the dependency declaration. No additional
`uses_effect`, `uses_source`, or manifest annotation exists.

### A capability is required to issue boundary work

The public Command and Subscription constructors require the matching
capability in addition to the inert descriptor:

```rust,ignore
Command::effect_with(&self.http, request, Message::HttpFinished)

Subscription::source_with(
    &self.input,
    SubscriptionId::new("input"),
    descriptor,
    Message::Input,
)
```

A raw `EffectDescriptor` cannot create an Effect Command, and a raw
`SourceDescriptor` cannot create a Subscription. The capability remains inert:
it performs no I/O, owns no Tokio handle, and exposes no live operation. It
authorizes only construction of runtime-interpreted data.

Cloning a capability does not declare another dependency. Every capability has
private Program provenance and a private identity allocated during assembly.
That identity is diagnostic and binding metadata only. It creates no execution
order, Component identity, or global serialization promise.

### Capability and descriptor have distinct roles

An Effect capability declares that the Program may issue descriptor type `D`.
Each Command still carries one concrete `D` value representing one finite
occurrence.

A Source capability declares that the Program may desire descriptor type `S`.
Each Subscription still carries the concrete, Model-derived `S` value used for
reconciliation. This preserves dynamic configuration such as reconnecting to a
different endpoint without dynamically changing the Program's dependency set.

Source capability identity is part of Source realization configuration. Equal
`ComponentId`, `SubscriptionId`, and SourceDescriptor values retain a Source
only when the capability is also unchanged. Selecting another capability under
the same Subscription identity replaces the Source, even when the descriptor
values compare equal. Mapper identity remains excluded from reconciliation.

### Composed Sources declare their terminal requirement once

`SourceCapability<S>` records both the application-visible descriptor type `S`
and the terminal descriptor type reached by Samara's sealed SourcePlan lowering.
For example, declaring `SourceCapability<Framed<TcpBytes, D>>` records
`TcpBytes` as the terminal binding requirement.

Applications do not separately declare the inner terminal descriptor or the
Layer stack. Live and controlled builders continue to bind only terminal
descriptors, and both profiles run the same Layers.

### Type-wide and exact bindings satisfy declared requirements

A normal `EffectDriver<D>` or `SourceDriver<D>` binding is type-wide. One such
binding may satisfy every declared capability whose terminal descriptor type is
`D`. Existing duplicate type-wide binding rejection remains in force.

Some adapters own one exact resource rather than an implementation for every
descriptor value. The first-party Tokio `mpsc` bridge is the initial example.
Its receiver binding is associated with one exact Source capability identity
whose terminal descriptor is `StreamDescriptor<T>`:

```rust,ignore
LiveRuntime::builder(program)
    .bind_mpsc(&input, receiver)
    .build()?;
```

Controlled assembly selects that same exact logical boundary with
`control_stream(&input)`. The capability may directly describe
`StreamDescriptor<T>` or a built-in composition such as
`Framed<StreamDescriptor<T>, D>`; terminal items and ending still pass through
the declared Layers. Runtime selection does not infer identity by comparing
future descriptor values. A type-wide binding and an exact binding that would
both satisfy one capability remain ambiguous and are rejected unless a future
ADR defines explicit precedence.

### Both execution profiles validate the complete declaration set

`LiveRuntimeBuilder::build` validates every Program-declared Effect and Source
capability before creating a live runtime. Each declaration must have exactly
one applicable terminal live binding. Missing, duplicate, ambiguous, foreign,
or type-incompatible bindings fail synchronously.

`ControlledRuntimeBuilder::build` performs the parallel validation. Every
declared terminal Effect and Source capability must have controlled behavior,
and exact capabilities must have their exact controlled binding. Controlled
assembly never falls back to a live Driver.

A boundary may remain unused in one execution, but its declared dependency must
still be satisfiable. Optional runtime behavior is not optional assembly.

After successful profile build, a conforming Component cannot first discover a
missing terminal Effect or Source binding in a later transition. Runtime checks
remain defense in depth for forged, foreign, or internally inconsistent values,
not the normal dependency-discovery mechanism.

### Cross-Program capabilities are invalid

A capability issued by one Program cannot authorize work in another Program.
Exact profile bindings reject foreign capability values during profile build.
Initial Commands and Subscriptions carrying foreign Effect, Source,
`ComponentRef`, or `Port` values are rejected during profile build. If
deliberately non-conforming Component code hides a foreign capability in an
arbitrary field and first issues it only after a later Message, runtime
interpretation faults with a provenance diagnostic before any Driver or
controlled behavior is invoked.

Rust cannot inspect arbitrary Component fields, just as it cannot prevent a
Component author from performing ambient I/O. The supported API makes correct
declaration the direct path and detects misuse wherever it becomes observable;
it does not claim to statically prove honest Component conformance.

### First-party helpers require capabilities too

Convenience APIs cannot recreate an undeclared path:

- HTTP response pipelines require an `EffectCapability<HttpRequest>` when
  lowered into a Command.
- `samara::print!`, `println!`, `eprint!`, and `eprintln!` require the matching
  standard-output capability as their first argument.
- Source helpers and exact bridge APIs require the matching Source capability.

Driver conveniences such as `bind_http`, `bind_stdio`, and `bind_tcp` remain
profile assembly operations. They install implementations; they do not declare
application dependencies.

## Consequences

### Positive

- Missing legitimate Effect and Source bindings become synchronous assembly
  errors, including dependencies first used after a later Message.
- Dependency declarations cannot drift from use because the declaration value
  is required at every issuance site.
- Component configuration becomes a readable inventory of its logical wiring.
- Exact resource adapters gain stable identity without changing descriptor
  equality or relying on future descriptor values.
- Live and controlled profile completeness is checked against the same closed
  Program declaration set.
- No macro analysis, typelist, restricted reducer language, or ambient runtime
  context is required.

### Negative

- Effect and Source issuance gains one explicit capability argument.
- Components must store logical capabilities even when they use a boundary in
  only one rare branch.
- First-party print macros become slightly less similar to Rust's ambient print
  macros because their world capability must remain visible.
- Deliberately non-conforming code can hide a foreign capability until runtime;
  ordinary Rust cannot provide complete field introspection.
- Existing applications and tests must migrate atomically because retaining raw
  constructors would preserve the bypass this ADR removes.

## Failure Modes

- Declared Effect or Source capability with no applicable profile binding:
  synchronous profile-build error.
- More than one applicable binding: synchronous profile-build error.
- Exact binding supplied a capability from another Program or of the wrong
  descriptor type: synchronous profile-build error.
- Command or Subscription carries a foreign or undeclared capability:
  synchronous profile-build error when visible initially, otherwise a runtime
  provenance fault before terminal behavior.
- A SourcePlan's actual terminal type disagrees with its capability's sealed
  declared terminal type: internal contract fault before starting a Source.
- One-shot exact resource is activated again after consumption: ADR-0004's
  existing runtime fault; build-time capability completeness does not make a
  one-shot resource restartable.

## Required Evidence

- A dependency used only after a later Message still makes live and controlled
  profile build fail when its declared terminal binding is absent.
- Bound capabilities execute unchanged in live and controlled profiles.
- Public compile-contract evidence proves raw EffectDescriptors cannot create
  Commands and raw SourceDescriptors cannot create Subscriptions.
- Initial foreign Effect, Source, ComponentRef, and Port capabilities and
  dynamically reached foreign Effect/Source capabilities are rejected before
  invoking a Driver or controlled behavior.
- A composed Source capability requires only its lowered terminal binding and
  runs the same Layers in both profiles.
- Exact `mpsc` live and controlled bindings select capability identity, reject
  foreign identities, preserve existing one-shot lifecycle behavior, and work
  beneath a composed Source whose terminal descriptor is `StreamDescriptor<T>`.
- Switching Source capability under an equal Subscription identity and equal
  descriptor replaces rather than retains the Source.
- Existing `ComponentRef`, `Port`, provider swapping, and PortHandle identity
  behavior remain coherent and topology-neutral.
- HTTP pipelines, standard-output macros, and all other first-party helpers have
  no raw undeclared issuance path.

## Explicitly Unresolved

- Named capabilities and user-facing capability identity inspection.
- Capability-specific Effect Drivers or precedence between exact and type-wide
  bindings.
- A compound standard-I/O capability or other bundles that reduce assembly
  fields without hiding individual requirements.
- Static derivation or linting of a Component's stored capability inventory.
- Dynamically adding Components, Ports, Effects, or Sources to a running
  Program; this ADR deliberately closes that door for the current architecture.
- Any blessed ambient or dynamic escape hatch and the guarantees it would
  forfeit.

## Rollback

If real applications show that capability threading is disproportionate, a
superseding ADR may restore a weaker dynamic-binding contract or introduce a
different non-drifting declaration mechanism. Rollback removes the capability
arguments and complete profile-build validation together; retaining optional
capabilities beside raw constructors would add ceremony without closing the
dependency graph.

The Program requirement registry and capability provenance are internal and
need no compatibility guarantee before 0.1. Observable ordering, causality,
Source cutover, structured ownership, and controlled determinism remain
unchanged by either adoption or rollback.
