# Exploratory Design: Subprograms

Status: implemented November 7, 2026 as additive API (`RuntimeBuilder`,
`Controlled`, `Bindings`, `Subprogram`, `Installed`, `WithSubprogram`,
`ProgramBuilder::install`), with
evidence in `tests/subprograms.rs`, the `Subprogram` rustdoc, and
`examples/pulse`. The follow-on that moves drivers into `install`'s
signature was considered and rejected; see "What remains open". Two findings
from landing it are under "What implementation taught".
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

- **Subprogram:** a reusable assembly unit that contributes Components, Ports,
  and binding requirements to an enclosing Program. It is subordinate to the
  Program, not to another Component. A Subprogram is usually one Component but
  may install several cooperating ones. The term replaces the descriptive
  "subprogram" already used in the follow-ups note.
- **Drivers (type):** the struct naming what a Subprogram needs bound, one
  field per declared effect or source capability, generic over the execution
  profile. A `Default` impl per profile lets hosts accept the Subprogram's
  shipped Drivers; the controlled impl is always `Controlled` markers.
- **RuntimeBuilder:** the profile-neutral binding surface shared by
  `LiveRuntimeBuilder` and `ControlledRuntimeBuilder`. It unifies only binding.
  Spawning, stepping, emitting source events, and supplying outcomes stay
  profile-specific and are deliberately not unified.

The name `SubComponent` was considered and rejected. It reads as a child inside
a parent Component's `update`, which is the nested-component model Samara does
not use; Components are peers connected by Ports.

## Shape

The authoritative description is the `Subprogram` rustdoc in `src/lib.rs`,
with its full example and two compile-fail cases, and `tests/subprograms.rs`.
In one paragraph: `RuntimeBuilder` is the profile-neutral binding surface
shared by the live and controlled builders; its generic associated types make
a live binding a boxed Driver and a controlled binding the `Controlled`
marker. `Bindings<R>` is implemented once per drivers struct, generic over the
profile. `Subprogram` has `Config`, `Exports`, and `Drivers<R>`, and an
`install` associated function that declares into a `ProgramBuilder`;
`ProgramBuilder::install` returns a `must_use` `Installed<S>` token carrying
the exports. On either builder, `with_subprogram(token)` returns a
`WithSubprogram` step whose only method, `and_drivers`, binds them and returns
the builder. Implement `Subprogram` on the protocol type, not the Component.
The pulse journal (`examples/pulse/src/journal.rs`) is the worked example.

## What it settles

- Drivers are named once, as a struct whose field names document them.
  No variadic generics are needed because the trait names one type per
  Subprogram, never a list of descriptors; the builders keep validating
  bindings by `TypeId` at build time.
- The link between installing and binding is both a type association and a
  value. Only `JournalDrivers` can be passed for `Journal`, adding a
  capability to the journal breaks every call site at compile time, and
  `with_subprogram` takes the `Installed<S>` token that only
  `ProgramBuilder::install` can create, so drivers cannot be bound for a
  Subprogram the Program never installed. The profile is the builder's own
  type, so the drivers literal needs no annotation in either profile.
- **A forgotten `and_drivers` does not compile inside a chain.**
  `with_subprogram` returns `WithSubprogram<B, S>`, which holds the builder and
  offers nothing but `and_drivers`. A chain that goes straight to
  `build()` or another binding method has no receiver for it. This is the only
  misuse the type system catches; the token lint below covers discards, and
  `build()` validation covers everything else. Earlier shapes, a free
  `Bound<S, R>` value and `with_subprogram(installed, drivers)`, caught
  this case only through `must_use`.
- The `must_use` token is a reminder, not a guarantee. Rust has no linear
  types, so the lint only fires when the token is discarded as a statement.
  Checked on stable 1.98:

  | Caller pattern | Warning |
  |---|---|
  | `program.install::<Journal>(());` as a statement | `unused_must_use` |
  | `let (port, token) = ...` with `token` never touched | `unused_variables` only |
  | `let (port, _) = ...` | none |
  | `let installed = ...; installed.exports();` and no `with_subprogram` | none |
  | `let _ = ...` | none |

  The fourth row is the realistic mistake, and only the builders' existing
  `build()` validation catches it. That is why the token carries the exports
  by reference and has no `into_exports`: an owned-exports escape hatch would
  reintroduce a forgettable path for no gain.
- Controlled tests never see a Driver type.
- Live and controlled builders share a binding trait without sharing a runtime
  interface. The earlier direction that the two runtimes share no interface is
  narrowed, not reversed: execution stays separate.

## What remains open

- **Drivers are deliberately not parameters of `install`.** Making them
  so would need one builder that owns both assembly and binding, which
  collapses the Program/Runtime divide: a Program is the closed, profile-neutral
  logical structure, and the live and controlled builders are two ways to run
  the same Program. That divide is what lets `main` and the controlled tests
  share one assembly, and it is what ADR-0008's two validation stages
  formalize. The gap between installing and binding is the price of that
  divide, and `Installed` plus `WithSubprogram` are the typed bridge across
  it: the token proves the Program side happened, and the waiting step forces
  the Runtime side to finish. Forgetting the bridge entirely is caught by
  `build()` with the descriptor named, which is the same contract `Result`
  offers through `must_use`. Decided November 7, 2026: not an end state to
  work toward.
- **Identity scoping.** `Config` is where a scope or instance id would go.
  Whether the runtime should help, for example by prefixing `PortId` and
  `ComponentId` under a Subprogram scope, is open.
- **Exports beyond a Port.** The url checker's tests wanted a private
  `ComponentRef` retained. Whether `Exports` should carry test-only handles,
  or whether that is what the isolated Component runtime follow-up is for, is
  open.
- **Boxing versus generics.** `Box<dyn EffectDriver<D>>` is the simple choice.
  A generic alternative would avoid the allocation at the cost of the
  drivers struct growing one type parameter per field, which defeats the
  point.

## What implementation taught

- **Implement `Subprogram` on the protocol type, not the Component.** The
  first pulse wiring implemented `Subprogram` on the Component (then named
  `FsJournal`, since renamed `Recorder` because the filesystem lives in the
  Driver), which made it a public name and failed to compile: `Component`'s associated
  `Model` and `Message` are part of its interface, so they would have to be
  public too. Implementing on `Journal`, the protocol type callers already
  hold, keeps the Component and its types private. The rustdoc example still
  shows the Component form because its types are all local to the example.
- **Controlled execution has no Port-level ingress.** `ControlledRuntime`
  delivers messages by `ComponentRef<C>` only, and inspects state the same way,
  so a Subprogram that exports just a Port cannot be driven or observed in a
  controlled test from outside its module. `tests/subprograms.rs` exports the
  `ComponentRef` alongside the Port, which works there only because the whole
  test is one module. For a real Subprogram the stopgap fails: `ComponentRef<C>`
  in `Exports` makes `C` public, which is the same `Model`/`Message` leak as
  implementing `Subprogram` on the Component. Until there is a Port-keyed
  controlled ingress, a Subprogram's isolated controlled tests live inside its
  module, where private types are reachable, as the url checker's do. ADR-0006
  made `PortHandle` live-only and named the preconditions for a controlled
  equivalent: deterministic scheduling, which controlled execution already has,
  and an explicit reply-observation model, such as a `PendingReply` token
  resolved after `run_until_idle` rather than an awaited future. That is an
  ADR extending 0003 and 0006, since host Port ingress becomes a traced root
  event in controlled execution.

## Relation to existing contracts

- Additive public API: one trait with two impls, one marker type, one trait
  for drivers, one trait for Subprograms, one method on `ProgramBuilder`,
  and one default method on `RuntimeBuilder`. No runtime semantics change, so
  ADR-0002, ADR-0003, and ADR-0004 are unaffected.
- ADR-0008 is unaffected by this sketch; its two-stage validation is what makes
  the omission backstop work. The follow-on that moves assembly into the
  runtime builder would revise it.
- Phase 6 froze its API slice under change control. This is additive and does
  not reshape frozen items, but the acceptance matrix and API guidance must
  cite it when it lands.
- The glossary needs entries for Subprogram and Drivers, and the
  "Composable Programs / subprograms" follow-up should reference this note.

## Evidence

`tests/subprograms.rs`, the `Subprogram` rustdoc examples, and the
`examples/pulse` journal; cited in the acceptance matrix. Rollback is
deleting the traits and types, after which callers assemble by hand and bind
Drivers directly.
