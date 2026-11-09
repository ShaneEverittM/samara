# Exploratory Design: Assemblies

Status: implemented November 7, 2026 as `Subprogram` with a profile-generic
drivers struct; redesigned and renamed November 8, 2026 as `Assembly` with a
type-level `Requires` list, accepted in
[ADR-0013](../adr/0013-component-kinds-assemblies-and-first-party-effects.md).
Evidence is in `tests/assemblies.rs`, the `Assembly` and `VendorsDrivers`
rustdoc, and `examples/pulse`. The follow-on that moves drivers into
`install`'s signature was considered and rejected; see "What remains open".
Findings from both passes are under "What implementation taught".
Motivated by `examples/pulse` and the
[install tension](framework-follow-ups.md#pulse-monitor-api-exercise--october-8-2026)
recorded there.

## Problem

A library-style Component such as the pulse journal is assembled by an
`install` function that declares its Component, Port, and capabilities into an
enclosing `ProgramBuilder` and exports only the Port. That keeps the Component,
its identities, and its capabilities private, and it is the working answer to
the composable-Programs follow-up. It leaves one gap: the module declared
effect capabilities, so somebody must bind a Driver for each one, and the
caller only learns which ones from the synchronous failure in
`LiveRuntime::builder(program).build()`.

Driver binding cannot move into `install`. Assembly is shared by live and
controlled execution, binding is an execution-profile decision, and controlled
execution must never run a live Driver. So the design question is how a
reusable assembly unit states its binding requirements so that the caller
cannot miss them, the statement is written once rather than once per profile,
and controlled tests never construct a Driver.

## Vocabulary

- **Assembly:** a reusable assembly unit that contributes Components, Ports,
  and binding requirements to an enclosing Program. It is subordinate to the
  Program, not to another Component. An Assembly is usually one Component but
  may install several cooperating ones. The term replaces the descriptive
  "assembly" already used in the follow-ups note.
- **Requires:** the type-level list naming what an Assembly needs driven:
  `Effect<D>` and `Source<S>` markers, `()`, or tuples of them. It is data the
  crate can read in both directions: control every entry on the controlled
  builder, and check a supplied driver tuple against it on the live builder.
- **Drivers (trait):** `Drivers<Requires>`, satisfied position by position by
  any `EffectDriver<D>` for `Effect<D>` and any `SourceDriver<S>` for
  `Source<S>`. Implemented by the crate for tuples; applications never
  implement it.
- **VendorsDrivers:** an optional subtrait naming and constructing concrete
  drivers for `Requires`, bound with `and_its_vendored_drivers()`. The one
  impl a library feature-gates.

The name `SubComponent` was considered and rejected. It reads as a child inside
a parent Component's `update`, which is the nested-component model Samara does
not use; Components are peers connected by Ports.

## Shape

The authoritative description is the `Assembly` rustdoc in `src/lib.rs`,
with its full example and two compile-fail cases, and `tests/assemblies.rs`.
In one paragraph: `Assembly` has `Config`, `Exports`, and `Requires`, and
an `install` associated function that declares into a `ProgramBuilder`;
`ProgramBuilder::install` returns a `must_use` `Installed<A>` token carrying
the exports. The live builder's `with_assembly(token)` returns a
`WithAssembly` step whose only method, `and_drivers`, takes any value
implementing `Drivers<Requires>` (a single driver or a tuple matching the
requirement tuple) and returns the builder. The controlled builder's
`with_assembly(token)` controls every requirement and returns the builder
directly. Implement `Assembly` on the protocol type, not the Component. The
pulse journal (`examples/pulse/src/journal.rs`) is the worked example: one
`type Requires` line and a four-line `VendorsDrivers` impl.

## What it settles

- Requirements are named once, as a type the crate can read. The builders
  keep validating bindings by `TypeId` at build time, and `Requires` adds a
  second check: a list that disagrees with what `install` declared fails
  `build()` on both profiles with the type named.
- The link between installing and binding is both a type association and a
  value. A driver at the wrong position is a compile error naming the
  expected `Effect<D>`, adding a requirement breaks every live call site at
  compile time, and `with_assembly` takes the `Installed<A>` token that only
  `ProgramBuilder::install` can create, so drivers cannot be bound for an
  Assembly the Program never installed.
- The controlled profile is never spelled by an Assembly or its tests. A
  controlled binding has no content beyond the descriptor type, and
  `Requires` already names that, so `with_assembly` on the controlled builder
  needs nothing further.
- **A forgotten `and_drivers` does not compile inside a chain.**
  `with_assembly` returns `WithAssembly<A>` live, which holds the builder and
  offers nothing but `and_drivers` and `and_its_vendored_drivers`. A chain that goes straight to
  `build()` or another binding method has no receiver for it. This is the only
  misuse the type system catches; the token lint below covers discards, and
  `build()` validation covers everything else. Earlier shapes, a free
  `Bound<S, R>` value and `with_assembly(installed, drivers)`, caught
  this case only through `must_use`.
- The `must_use` token is a reminder, not a guarantee. Rust has no linear
  types, so the lint only fires when the token is discarded as a statement.
  Checked on stable 1.98:

  | Caller pattern | Warning |
  |---|---|
  | `program.install::<Journal>(());` as a statement | `unused_must_use` |
  | `let (port, token) = ...` with `token` never touched | `unused_variables` only |
  | `let (port, _) = ...` | none |
  | `let installed = ...; installed.exports();` and no `with_assembly` | none |
  | `let _ = ...` | none |

  The fourth row is the realistic mistake, and only the builders' existing
  `build()` validation catches it. That is why the token carries the exports
  by reference and has no `into_exports`: an owned-exports escape hatch would
  reintroduce a forgettable path for no gain.
- Controlled tests never see a Driver type.
- Live and controlled builders share no trait. The first pass introduced a
  profile-neutral `RuntimeBuilder`; the second removed it, because once
  `Requires` is data the two builders need nothing in common.

## What remains open

- **Drivers are deliberately not parameters of `install`.** Making them
  so would need one builder that owns both assembly and binding, which
  collapses the Program/Runtime divide: a Program is the closed, profile-neutral
  logical structure, and the live and controlled builders are two ways to run
  the same Program. That divide is what lets `main` and the controlled tests
  share one assembly, and it is what ADR-0008's two validation stages
  formalize. The gap between installing and binding is the price of that
  divide, and `Installed` plus `WithAssembly` are the typed bridge across
  it: the token proves the Program side happened, and the waiting step forces
  the Runtime side to finish. Forgetting the bridge entirely is caught by
  `build()` with the descriptor named, which is the same contract `Result`
  offers through `must_use`. Decided November 7, 2026: not an end state to
  work toward.
- **Identity scoping.** `Config` is where a scope or instance id would go.
  Whether the runtime should help, for example by prefixing `PortId` and
  `ComponentId` under an Assembly scope, is open.
- **Exports beyond a Port.** The url checker's tests wanted a private
  `ComponentRef` retained. Whether `Exports` should carry test-only handles,
  or whether that is what the isolated Component runtime follow-up is for, is
  open.
- **Named positions.** `Requires` is positional. Past five or six entries a
  host's `and_drivers((a, b, c, d, e, f))` stops reading well, which is also
  a signal that the Assembly is too large. A `Requires`-level grouping that
  one driver satisfies, such as an `FsDriver` for several filesystem effects,
  is the expected extension; a named-field form was considered and deferred.
- **Vendored-driver discovery.** `VendorsDrivers` gives rustdoc a name to
  find. Whether hosts actually find it, or whether a module-level convention
  would have sufficed, is a question for the second Assembly that vendors.

## What implementation taught

- **Implement `Assembly` on the protocol type, not the Component.** The
  first pulse wiring implemented `Assembly` on the Component (then named
  `FsJournal`, since renamed `Recorder` because the filesystem lives in the
  Driver), which made it a public name and failed to compile: `Component`'s associated
  `Model` and `Message` are part of its interface, so they would have to be
  public too. Implementing on `Journal`, the protocol type callers already
  hold, keeps the Component and its types private. The rustdoc example still
  shows the Component form because its types are all local to the example.
- **Controlled execution has no Port-level ingress.** `ControlledRuntime`
  delivers messages by `ComponentRef<C>` only, and inspects state the same way,
  so an Assembly that exports just a Port cannot be driven or observed in a
  controlled test from outside its module. `tests/assemblies.rs` exports the
  `ComponentRef` alongside the Port, which works there only because the whole
  test is one module. For a real Assembly the stopgap fails: `ComponentRef<C>`
  in `Exports` makes `C` public, which is the same `Model`/`Message` leak as
  implementing `Assembly` on the Component. Until there is a Port-keyed
  controlled ingress, an Assembly's isolated controlled tests live inside its
  module, where private types are reachable, as the url checker's do. ADR-0006
  made `PortHandle` live-only and named the preconditions for a controlled
  equivalent: deterministic scheduling, which controlled execution already has,
  and an explicit reply-observation model, such as a `PendingReply` token
  resolved after `run_until_idle` rather than an awaited future. That is an
  ADR extending 0003 and 0006, since host Port ingress becomes a traced root
  event in controlled execution.

- **The drivers struct was pure derivation.** Its `Bindings` impl called
  one bind method per field and its controlled `Default` was a row of
  identical markers; neither carried a decision. Making the declaration a
  type (`Requires`) instead of a struct let the crate derive both, and
  removed `RuntimeBuilder`, `Bindings`, and `Controlled` with them. Coherence
  allows the two driver blankets because the kind lives on the marker
  (`Effect<D>` versus `Source<S>`), not on the descriptor trait; blankets over
  `EffectDescriptor` and `SourceDescriptor` would overlap.
- **Vendored drivers are a free function or a subtrait, not a trait item.**
  A required `drivers()` on `Assembly` would force every Assembly to answer,
  and a gated trait item is impossible. A free function was the first form;
  `VendorsDrivers` replaced it for discoverability, at the cost of one more
  concept.

## Relation to existing contracts

- Additive public API apart from the kernel signature, which ADR-0013
  records: `Assembly`, `Installed`, `WithAssembly`, `Effect`, `Source`,
  `Requirements`, `Drivers`, `VendorsDrivers`, `ProgramBuilder::install`, and
  `with_assembly` on each builder. No runtime semantics change, so ADR-0002,
  ADR-0003, and ADR-0004 are unaffected.
- ADR-0008 is unaffected by this sketch; its two-stage validation is what makes
  the omission backstop work. The follow-on that moves assembly into the
  runtime builder would revise it.
- Phase 6 froze its API slice under change control. This is additive and does
  not reshape frozen items, but the acceptance matrix and API guidance must
  cite it when it lands.
- The glossary has entries for Assembly, Requires, and VendorsDrivers; the
  "Library assembly via `install`" follow-up references this note.

## Evidence

`tests/assemblies.rs`, the `Assembly` and `VendorsDrivers` rustdoc examples,
and the `examples/pulse` journal; cited in the acceptance matrix. Rollback is
deleting the traits and types, after which callers assemble by hand and bind
Drivers directly.
