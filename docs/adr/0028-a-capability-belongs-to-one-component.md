# ADR-0028: A Capability Belongs to One Component

- Status: Accepted and implemented. Completed by
  [ADR-0029](0029-capabilities-are-named-values-a-component-owns.md), which
  also names capabilities and settles the spellings left open here. The
  "Implementation status" section records the interim first half.
- Date: November 15, 2026
- Deciders: Samara maintainers
- Amends: [ADR-0008](0008-closed-program-capabilities.md) and
  [ADR-0019](0019-typed-program-requirements.md) (the Program's requirement
  type and up-front declaration),
  [ADR-0022](0022-file-resource-capabilities.md) (narrowing a file
  capability for another Component),
  [ADR-0026](0026-assemblies-export-one-value.md) (what `install` receives
  and returns), and
  [ADR-0027](0027-drivers-are-supplied-per-capability.md) (`with` on
  registration, `Program::capabilities`)
- Scope: Where capabilities are created; who may hold one; what an installed
  Component or Assembly hands the host; per-capability Driver binding for
  plain effects and sources

## Context

`Program::builder::<R>()` declares every capability up front as one typed
tuple and returns the values, which the host then distributes with
`with(&capabilities)`. Three things follow from that shape.

- **Everything is declared in one place.** An application composes every
  module's `Requires` into one type alias and destructures one tuple, which
  is the last positional step left after ADR-0027. Installation cannot be
  moved into per-module functions without threading those values through.
- **`R` no longer checks anything.** It existed so the compiler could match
  a driver tuple against it. ADR-0027 moved that check to `build()`.
- **Sharing is possible and unspecified.** Capability values are `Clone`, so
  one can be given to several Components, and a `File` capability can be
  narrowed and handed on. Nothing says what two Components appending to one
  file, or reading while another appends, are promised. The pulse journal
  already had to document that reads have no ordering against appends.

A resource needs a single owner to have well-defined semantics. That is the
actor principle Samara otherwise follows: state has one owner and everyone
else sends it a Message.

## Decision

1. **Registration creates a Component's capabilities.** There is no
   up-front declaration. `Program::builder()` takes no requirement type and
   `Program` has none.
2. **A capability belongs to the Component it was created for.** Nothing
   accepts an existing capability at registration, so one cannot be given to
   a second Component. `with(..)` is removed from Component and Assembly
   registration, and the `File` narrowing methods are removed.
3. **The installed handle carries the capabilities for the host.**
   Registering a Component returns a host-side handle with
   `capabilities()` and the `ComponentRef`. A `ComponentRef` stays a plain
   address with no authority on it, because refs are cloned into other
   Components' Configs.
4. **An Assembly's capabilities are its members' capabilities.** `install`
   no longer receives capabilities. Each Component it registers gets its
   own, and the Assembly returns them with its exports so the host can
   supply Drivers. `Requires` remains the type the host sees.
5. **Every capability is bound to its own Driver.** Plain `Effect<D>` and
   type-wide `Source<S>` Drivers are bound per capability, as `File<P>` and
   exact sources already are. Two Components that both read the clock have
   two capabilities and are each supplied a Driver.
6. **Using a capability you do not own is a runtime fault**, the same
   defense that already rejects a capability from another Program. This
   closes the remaining path, a host copying a capability into another
   Component's Config.

```rust,ignore
let mut builder = Program::builder();
let journal = builder.install::<Journal>(()).finish()?;
let monitor = builder
    .component::<Monitor>(Config { journal: journal.connector().clone() })
    .finish()?;

Runtime::builder(builder.build()?)
    .with(journal.capabilities()).supplied_by((FileDriver::new(path), SystemClockDriver))
    .with(monitor.capabilities()).supplied_by((StdinDriver, StdoutDriver::new()))
    .build()?;
```

### Sharing a resource

Whether two capabilities are backed by one real resource is the host's
decision, made when it supplies Drivers: one `HttpDriver` cloned to two
Components shares a connection pool. The Program does not express it.

Two Components that need the same file do not each hold it. One Component
owns the file and the others send it Messages. An Assembly that wants an
ingest side and a read side over one store has a store Component that owns
the file, addressed by both.

## Alternatives considered

- **Keep `with(&capabilities)` as an escape hatch for sharing.** Preserves
  today's tests that share or narrow a capability, and keeps the
  unspecified semantics with them.
- **An incremental `builder.declare::<R>()` with registration unchanged.**
  Removes the up-front tuple but leaves capabilities as free values that
  can be shared, and every module helper needs a struct to return them in.
- **Extend the builder's type on each installation.** Keeps a type-level
  record nothing consumes, forces the builder to be threaded by value
  through generic helpers, and rules out conditional or repeated installs.
- **Put capabilities on `ComponentRef`.** Rejected in item 3.

## Consequences

- An application names no requirement type and destructures no tuple. The
  per-module helper with a named result struct is an Assembly: `install` is
  the helper and `Exports` is the struct.
- The Program is closed when `build()` returns, not by its type. The
  compile-fail cases for adding a capability after choosing the type go
  away. Completeness of Drivers is still checked by the live `build()`.
- A capability has an owner, so the missing-Driver error can name the
  Component as well as the descriptor type.
- An Assembly can no longer hand one file to two of its Components with
  different permissions. See "Sharing a resource".
- The host supplies a Driver per Component for common effects such as the
  clock or standard output. Two `StdoutDriver`s serialize their own writes
  independently, so a host that needs one ordering shares one Driver.
- Controlled execution must derive its controls from the Program's recorded
  requirements instead of from `R`.
- Every registration site, every Assembly, and the tests that share or
  narrow capabilities change.
- No change to transition purity, per-Component serialization, causal
  ordering, or controlled determinism.

## Implementation status

Landed first, beside the existing API, so the host-facing shape can be
lived with before every usage moves:

- `ProgramBuilder::new()` starts a Program that declares nothing up front.
- A bare `finish()` on Component or Assembly registration creates that
  registration's capabilities. A Component returns `InstalledComponent<C>`
  with `capabilities()` and `reference()`; it derefs to its `ComponentRef`
  and is `Addressable`. A Component that requires nothing still returns its
  plain `ComponentRef`. `InstalledAssembly` gains `capabilities()`.
- An `Effect<D>` capability created this way is bound to its own Driver, so
  two Components with the same effect are each supplied one.
- `control_all()` derives its controls from the Program's recorded
  requirements, so it covers capabilities created at registration.

Not yet done, and required before this ADR is fully in force:

- `Program::builder::<R>()`, `Program<R>`, `with(&capabilities)` on
  registration, `Program::capabilities()`, and the `File` narrowing methods
  still exist, so sharing is still possible through the old path.
- `Assembly::install` still receives its capabilities from the builder and
  hands them on with `with(&capabilities)` (item 4).
- Type-wide `Source<S>` Drivers are still bound per descriptor type, so two
  Components with the same type-wide source cannot each be supplied one.
- Using a capability you do not own is not yet a fault (item 6).
- The missing-Driver error does not yet name the owning Component.

## Open at implementation

- The spelling of what `install` returns for its members' capabilities, and
  the name of the Component handle.
- Whether controlled tests that select a pending effect by descriptor type
  need to select by Component as well.
- `Program::capabilities()` is removed; test fixtures return their handles.

## Rollback

Revert the implementing commits. Up-front declaration, `Program<R>`, and
`with(&capabilities)` on registration return.
