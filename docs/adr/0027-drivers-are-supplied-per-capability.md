# ADR-0027: Drivers Are Supplied per Capability Value

- Status: Accepted. The spelling is amended by
  [ADR-0029](0029-capabilities-are-named-values-a-component-owns.md):
  `supply(&capability, driver)` replaces `with(&capability).supplied_by(driver)`,
  registration no longer takes capabilities at all, and
  `Program::capabilities()` is removed.
  Extended November 15, 2026: `supply_default(&capability)` supplies the
  standard Driver for a capability whose descriptor has an obvious one
  (stdin, stdout, stderr, the system clock, HTTP, TCP). It is `supply` with
  the Driver filled in from the capability's type: still one explicit call
  per capability or tuple, still exactly one Driver per capability, and
  nothing is supplied that the host did not name. A capability whose Driver
  needs a setting, such as a `File`, has none. The set is first-party and
  sealed for now.
- Date: November 10, 2026
- Deciders: Samara maintainers
- Amends: [ADR-0019](0019-typed-program-requirements.md) (live binding and
  its compile-time completeness; registration's `with_capabilities(caps)`
  becomes `with(&capabilities)`)
- Scope: `RuntimeSetup::with`, `RuntimeSupply::supplied_by`, and
  `RuntimeSetup::build`; removal of `with_drivers`; registration's
  `with_capabilities` renamed `with` and taking a borrow;
  `Program::capabilities`

## Context

ADR-0019 binds live Drivers with one tuple that mirrors the Program's
requirement shape position by position:

```rust,ignore
Runtime::builder(program).with_drivers((
    (FileDriver::new("./data/pulse.journal"), SystemClockDriver),
    (StdinDriver, StdoutDriver::new()),
))
```

That gives a compile-time guarantee that every position has a Driver of the
right type. It has three costs that grow with the application.

- **Silent swaps.** Two positions of the same type accept each other's
  Drivers. A Program with a journal file and a config file, both
  `File<ReadAppend>`, can have their paths exchanged with no error at any
  stage. The capability destructure at `Program::builder` is positional in
  the same way.
- **Distance.** The tuple must mirror a type declared elsewhere, and a shape
  mismatch is reported against the whole tuple.
- **Size.** One expression carries every Driver in the application.

Named requirement structs would fix all three and keep the compile-time
check, but they need per-struct generated code (a macro) for the
requirement, capability, and driver types. A typestate builder taking one
call per position is possible without variadic generics but stays
positional, and an order-free typestate cannot tell two same-typed
positions apart, which is the case that matters.

## Decision

The runtime builder names a capability value, or any tuple of them, and is
then given its Drivers. Pairs are supplied in any order. `with_drivers` is
removed.

```rust,ignore
let (mut builder, (journal_caps, monitor_caps)) = Program::builder::<AppRequirements>();
let journal = builder.install::<Journal>(()).with(&journal_caps).finish()?;
builder.component::<Monitor>(config).with(&monitor_caps).finish()?;

Runtime::builder(builder.build()?)
    .with(&journal_caps).supplied_by((FileDriver::new(path), SystemClockDriver))
    .with(&monitor_caps).supplied_by((StdinDriver, StdoutDriver::new()))
    .build()?;
```

- `with(&capabilities)` returns a stage whose only method is
  `supplied_by(drivers)`, which returns the builder. This is the staged
  shape registration already uses (`component::<C>(config)
  .with(&caps).finish()`). A named capability left without
  Drivers does not compile, and the stage is `must_use`.
- `Capability` is a sealed trait implemented for every shape
  `Program::builder` hands out: `EffectCapability<D>`,
  `SourceCapability<S>`, `FileCapability<P>`, `()`, and tuples of those. It
  is only a bound; `supplied_by` requires `Drivers` for exactly the named
  shape, so a Driver of the wrong type is still a compile error.
- Registration's `with_capabilities(caps)` is renamed `with(&capabilities)`
  on both Component and Assembly registration and takes
  `&Capabilities<_>`. Registration and the runtime builder then take the
  same value through the same word, and a call no longer says
  "capabilities" twice (`.with_capabilities(&journal_capabilities)`). The host keeps the values, so it can name a tuple
  whole or destructure it and name each part separately.
- `RuntimeSetup::build` exists for every requirement shape. It fails when a
  declared capability has no Driver, has more than one, or when a named
  capability belongs to another Program. The error names the descriptor
  type. This is the validation the runtime already performed after
  `with_drivers`; it is now the completeness check.
- `Program::capabilities()` returns the Program's whole capability tuple,
  for code that builds a runtime from a Program it did not assemble.
- `RuntimeBuilder` is no longer public; `RuntimeSetup` is the builder.

The Driver is tied to the capability value, and that same value is what a
Component is registered with. Which position the value came from no longer
matters: whichever file capability the journal was given is the one the
journal's `FileDriver` is paired with.

## What is traded

| | `with_drivers` (ADR-0019) | `with(..).supplied_by(..)` |
| --- | --- | --- |
| Driver type matches capability | compile time | compile time |
| Every capability supplied | compile time | `build()` |
| No capability supplied twice | `build()` | `build()` |
| Same-typed positions swapped | never detected | cannot happen by position |

The missing-Driver error moves from the compiler to the first live
`build()`. It is deterministic and does not depend on which messages
arrive. Controlled execution binds no Drivers and cannot observe it, so an
application that wants it caught in CI builds its live runtime in a test.

## Alternatives considered

- **Named requirement structs through a macro.** Keeps the compile-time
  check and adds names. Still open as a later addition; it composes with
  this decision, since a named capabilities struct can implement
  `Capability`.
- **A positional typestate builder**, one `with_drivers` call per top-level
  position. Better error locality, same swap hazard.
- **`with_capability(capabilities.supplied_by(drivers))`.** Implemented
  first. `supplied_by` was a trait method on capability values, so the trait
  had to be in scope at every host that does not import the prelude, and
  the pairing was a public value (`BoundCapability`) with no use except
  being handed to the builder. The staged form needs neither.
- **Keeping `with_drivers` beside the new form.** Two ways to bind, one of
  which has the swap hazard. A whole positional tuple is still expressible
  as `with(&all_capabilities).supplied_by(tuple)` when a host wants it.

## Consequences

- ADR-0019's statement that a Program with unbound requirements cannot
  reach runtime construction no longer holds. The rustdoc compile-fail case
  for a bare `build()` becomes a runtime-error example.
- Every `with_drivers` call site and every `with_capabilities` call site
  changes. Mechanical.
- Naming a whole tuple and supplying a whole tuple is as positional as
  `with_drivers` was. The swap protection comes from naming capabilities
  separately, which the API now allows and the examples do.
- Plain `Effect<D>` and type-wide Source bindings remain type-wide: one
  Driver serves every capability of that descriptor type, and supplying two
  is a duplicate error. Binding by value makes per-capability Drivers for
  plain effects a natural next step; it is recorded as a follow-up and is
  not part of this decision. `File<P>` and exact sources are already bound
  per capability.
- No change to transition purity, serialization, ordering, controlled
  determinism, or what a Driver may do.

## Evidence

`tests/capability_binding.rs`: two same-typed files named against
declaration order each receive the Driver supplied for their value; a tuple
named whole and split after registration borrowed it; a missing Driver, a
duplicate, and a foreign capability each fail `build` with a naming error.
Every example and live test uses the new form. The `RuntimeSetup` rustdoc
carries the passing example, the runtime-error example, and compile-fail
cases for a mismatched Driver type and for a named capability left without
Drivers.

## Rollback

Revert the implementing commits: `with_drivers` and the public
`RuntimeBuilder` return. Reverting the rename and borrow on registration is a
separate mechanical change.
