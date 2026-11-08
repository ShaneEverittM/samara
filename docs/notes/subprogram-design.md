# Exploratory Design: Subprograms

Status: implemented November 7, 2026 as additive API (`RuntimeBuilder`,
`Controlled`, `Bindings`, `Subprogram`, `Installed`, `WithSubprogram`,
`ProgramBuilder::install`), with
evidence in `tests/subprograms.rs`, the `Subprogram` rustdoc, and
`examples/pulse`. The follow-on that moves drivers into `install`'s
signature remains exploratory. Two findings from landing it are recorded under
"What implementation taught" below.
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

## Sketch

### `RuntimeBuilder`

The one hard difference between profiles is that a live binding carries a
Driver and a controlled binding carries nothing. A generic associated type
holds that difference so a Subprogram can name a binding without naming the
profile.

```rust
pub trait RuntimeBuilder: Sized {
    type EffectBinding<D: EffectDescriptor>;
    type SourceBinding<S: SourceDescriptor>;

    fn bind_effect<D: EffectDescriptor>(self, binding: Self::EffectBinding<D>) -> Self;
    fn bind_source<S: SourceDescriptor>(self, binding: Self::SourceBinding<S>) -> Self;

    fn with_subprogram<S: Subprogram>(self, installed: Installed<S>) -> WithSubprogram<Self, S>;
}

#[must_use]
pub struct WithSubprogram<B: RuntimeBuilder, S: Subprogram> { builder: B, .. }

impl<B: RuntimeBuilder, S: Subprogram> WithSubprogram<B, S> {
    pub fn and_drivers(self, drivers: S::Drivers<B>) -> B;
}

/// Marker binding: controlled execution supplies outcomes through the harness.
pub struct Controlled;

impl RuntimeBuilder for LiveRuntimeBuilder {
    type EffectBinding<D: EffectDescriptor> = Box<dyn EffectDriver<D>>;
    type SourceBinding<S: SourceDescriptor> = Box<dyn SourceDriver<S>>;
    // Both methods delegate to the existing generic `bind_effect` / `bind_source`,
    // which requires `impl EffectDriver<D> for Box<dyn EffectDriver<D>>`.
}

impl RuntimeBuilder for ControlledRuntimeBuilder {
    type EffectBinding<D: EffectDescriptor> = Controlled;
    type SourceBinding<S: SourceDescriptor> = Controlled;
    // Both methods delegate to `control_effect` / `control_source`.
}
```

`EffectDriver` and `SourceDriver` are already object-safe, so boxing costs one
allocation per binding at startup. The existing generic `bind_effect` and
`bind_source` remain for direct callers; the trait methods are a second path,
not a replacement.

### `Bindings`

```rust
pub trait Bindings<R: RuntimeBuilder> {
    fn bind(self, runtime: R) -> R;
}
```

A Subprogram implements this once for its drivers struct, generic over
`R`. Live and controlled are then the same code path with different field
types.

### `Subprogram`

```rust
pub trait Subprogram {
    type Config;
    type Exports;
    type Drivers<R: RuntimeBuilder>: Bindings<R>;

    fn install(program: &mut ProgramBuilder, config: Self::Config) -> Self::Exports;
}

#[must_use]
pub struct Installed<S: Subprogram> { exports: S::Exports, .. }

impl<S: Subprogram> Installed<S> {
    pub fn exports(&self) -> &S::Exports;
}

impl ProgramBuilder {
    pub fn install<S: Subprogram>(&mut self, config: S::Config) -> Installed<S>;
}
```

`install` on the trait is an associated function, not a method, because it constructs the
Component from capabilities the builder issues. `Config` carries whatever the
caller must supply at assembly, including identity when multiple instances are
possible. `Exports` is what the caller may use afterward, normally a `Port`;
`ComponentRef` and capabilities stay private unless deliberately exported.

The trait is separate from `Component` and does not require the implementor to
be one. Application-owned Components such as the pulse monitor never implement
it, so they pay nothing. A module that installs several Components implements
it on a marker type or on its protocol type.

### Worked example: the pulse journal

```rust
pub struct JournalDrivers<R: RuntimeBuilder> {
    pub append: R::EffectBinding<AppendLine>,
    pub clock: R::EffectBinding<Now>,
}

impl<R: RuntimeBuilder> Bindings<R> for JournalDrivers<R> {
    fn bind(self, runtime: R) -> R {
        runtime.bind_effect::<AppendLine>(self.append).bind_effect::<Now>(self.clock)
    }
}

impl JournalDrivers<ControlledRuntimeBuilder> {
    pub fn controlled() -> Self { Self { append: Controlled, clock: Controlled } }
}

impl Subprogram for Journal {
    type Config = ();
    type Exports = Port<Journal>;
    type Drivers<R: RuntimeBuilder> = JournalDrivers<R>;

    fn install(program: &mut ProgramBuilder, _: ()) -> Port<Journal> {
        let append = program.effect::<AppendLine>();
        let clock = program.effect::<Now>();
        let port = program.port::<Journal>(PortId::new("pulse/journal"));
        let component = program.component(
            ComponentId::new("fs-journal"),
            FsJournal { append, clock },
        );
        program.bind_port(&port, &component);
        port
    }
}
```

```rust
// Live host.
let mut program = Program::builder();
let journal = program.install::<Journal>(());
// ... assemble the application's own Components against `journal.exports()` ...
let runtime = LiveRuntime::builder(program.build()?)
    .with_subprogram(journal)
    .and_drivers(JournalDrivers {
        append: Box::new(FsAppend::new(path)),
        clock: Box::new(SystemClock),
    })
    .bind_http()
    .bind_stdio()
    .bind_stdin()
    .build()?;

// Controlled test.
let runtime = ControlledRuntime::builder(program.build()?)
    .with_subprogram(journal)
    .and_drivers(JournalDrivers::default())
    .build()?;
```

## What the sketch settles

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

## What the sketch does not settle

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
  first pulse wiring put `impl Subprogram for FsJournal`, which made
  `FsJournal` a public name and failed to compile: `Component`'s associated
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

## Acceptance evidence before implementation

- Library unit tests: `with_subprogram` on both builders binds every
  requirement; a live build with a Subprogram installed and no
  `with_subprogram` call fails with the descriptor type named; a controlled
  build with `Drivers::default()` registers every descriptor.
- `examples/pulse` as the first Subprogram, exercised live and in controlled
  tests, with the journal's two Drivers bound only in `main`.
- A compile-fail doctest showing that `JournalDrivers` cannot be passed
  for a different Subprogram.
- Rollback: delete the traits and the method; `install` reverts to a free
  function and callers bind Drivers directly. No data or semantics to recover.
