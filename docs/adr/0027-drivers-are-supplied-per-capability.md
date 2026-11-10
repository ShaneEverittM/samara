# ADR-0027: Drivers Are Supplied per Capability Value

- Status: Accepted for implementation trial. `with_drivers` remains beside
  the new form until the trial decides whether it is still wanted.
- Date: November 10, 2026
- Deciders: Samara maintainers
- Amends: [ADR-0019](0019-typed-program-requirements.md) (live binding and
  its compile-time completeness; `with_capabilities` takes a borrow)
- Scope: `Capability::supplied_by`, `BoundCapability`,
  `RuntimeSetup::with_capability` and `RuntimeSetup::build`; the receiver
  type of `with_capabilities`

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

A capability value, or any tuple of them, is paired with its Drivers, and
the pairs are handed to the runtime builder in any order.

```rust,ignore
let (mut builder, (journal_caps, monitor_caps)) = Program::builder::<AppRequirements>();
let journal = builder.install::<Journal>(()).with_capabilities(&journal_caps).finish()?;
builder.component::<Monitor>(config).with_capabilities(&monitor_caps).finish()?;

Runtime::builder(builder.build()?)
    .with_capability(journal_caps.supplied_by((FileDriver::new(path), SystemClockDriver)))
    .with_capability(monitor_caps.supplied_by((StdinDriver, StdoutDriver::new())))
    .build()?;
```

- `Capability` is a sealed trait implemented for every shape
  `Program::builder` hands out: `EffectCapability<D>`,
  `SourceCapability<S>`, `FileCapability<P>`, `()`, and tuples of those.
  `supplied_by(&self, drivers)` requires `Drivers` for exactly that shape, so
  a Driver of the wrong type is still a compile error.
- `with_capabilities` on Component and Assembly registration takes
  `&Capabilities<_>`. The host keeps the values, so it can supply a tuple
  whole or destructure it and supply each part separately.
- `RuntimeSetup::build` exists for every requirement shape. It fails when a
  declared capability has no Driver, has more than one, or when a supplied
  capability belongs to another Program. The error names the descriptor
  type. This is the validation the runtime already performed after
  `with_drivers`; it is now the only completeness check on this path.

The Driver is tied to the capability value, and that same value is what a
Component is registered with. Which position the value came from no longer
matters: whichever file capability the journal was given is the one the
journal's `FileDriver` is paired with.

## What is traded

| | `with_drivers` (ADR-0019) | `with_capability` |
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
- **`RuntimeSetup::drive(&capabilities, drivers)`.** Same semantics in one
  call. `supplied_by` was preferred because the pairing can be made next to
  the registration it belongs to and carried to the runtime builder.

## Consequences

- ADR-0019's statement that a Program with unbound requirements cannot
  reach runtime construction holds only for `with_drivers`. The rustdoc
  compile-fail case for a bare `build()` becomes a runtime-error example.
- Every `with_capabilities` call site gains a `&`. Mechanical.
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
declaration order each receive the Driver paired with their value; a tuple
supplied whole and split after registration borrowed it; a missing Driver, a
duplicate, and a foreign capability each fail `build` with a naming error.
`examples/pulse` uses the new form. The `Capability` and `RuntimeSetup`
rustdoc carry the passing example, the runtime-error example, and the
compile-fail case for a mismatched Driver type.

## Rollback

Remove `Capability`, `BoundCapability`, `with_capability`, and the general
`build`; restore `build` for the empty shape only. Reverting the borrow on
`with_capabilities` is a separate mechanical change.
