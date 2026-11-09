# ADR-0013: Component Kinds, Assemblies, and Provisional First-Party Effects

- Status: Accepted
- Date: November 8, 2026
- Deciders: Samara maintainers
- Scope: The `Component` trait signature, the reuse unit (`Assembly`), Port
  binding conversion, and first-party clock and filesystem effects

## Context

Three pressures from the `examples/pulse` developer-experience trial met in
one day of design.

The Component trait mixed two things under `&self`. The type you implement
`Component` on was also the value holding its immutable wiring, so "the
Recorder" named the kind, the configuration, and (in prose) the running thing.
The glossary already managed the ambiguity with separate terms, "Component
implementation" and "Component configuration", for one Rust type.

The reuse unit, then named `Subprogram`, declared what a host must drive as a
struct with one field per capability, generic over the execution profile
through a `RuntimeBuilder` trait, plus a `Bindings` impl that called the
matching bind method per field and a `Default` impl per profile. Every impl
was derivable from the field list and none carried information; the
controlled impl was a row of identical `Controlled` markers.

Pulse's journal appended lines and read the clock through hand-rolled
descriptors and drivers, the stopgap the filesystem design note names, and
every host would repeat them.

## Decision

### Component kind, configuration, and state are three types

```rust,ignore
pub trait Component: 'static {
    type Config: Send + 'static;
    type Model: Send + 'static;
    type Message: Send + 'static;
    fn init(config: &Self::Config) -> Init<Self::Model, Self::Message>;
    fn update(config: &Self::Config, model: &mut Self::Model, message: Self::Message)
        -> Command<Self::Message>;
    fn subscriptions(config: &Self::Config, model: &Self::Model) -> Subscriptions<Self::Message>;
}
```

The implementing type is only a name for the kind and is never stored. Its
wiring (capabilities, Ports, settings) is `Config`, supplied at registration
and held by the kernel; its behavioral state is `Model`. Registration names
the kind explicitly, `component::<Recorder>(id, config)`, because `C` cannot
be inferred from `C::Config`. Dynamic dispatch over `Component` is not
supported; it was already unreachable in practice, since the trait's
associated types have no defaults, and Ports are the dispatch mechanism.

### An Assembly declares its requirements as a type

`Subprogram` is renamed `Assembly`: a reusable slice of Program assembly. A
`Program` is the closed assembled graph, not the running thing, so the name
carries no runtime-hierarchy claim.

```rust,ignore
pub trait Assembly {
    type Config;
    type Exports;
    type Requires: Requirements;
    fn install(program: &mut ProgramBuilder, config: Self::Config) -> Self::Exports;
}
```

`Requires` is a type-level list: an `Effect<D>` or `Source<S>` marker, `()`,
or a tuple of lists (nesting allowed, tuple impls to arity eight). The live
builder's `with_assembly(installed).and_drivers(drivers)` accepts any value
implementing `Drivers<Requires>`: an `EffectDriver<D>` satisfies `Effect<D>`,
a `SourceDriver<S>` satisfies `Source<S>`, and a tuple satisfies a tuple
position by position. The controlled builder's `with_assembly(installed)`
controls every entry and takes nothing, because a controlled binding carries
no information beyond the descriptor type. An optional `VendorsDrivers:
Assembly` names and constructs concrete drivers, bound with
`and_its_vendored_drivers()`; libraries are expected to feature-gate that
impl. `RuntimeBuilder`, `Bindings`, and `Controlled` are removed.

### Port binding has an explicit-conversion form

`ProgramBuilder::connect_with(&port, &provider, map)` takes a pure
`Fn(P::Message) -> C::Message`, normally the enum variant wrapping the protocol
message, following the paired convention of `effect_with` and `source_with`.
`connect` remains the `From` shortcut.

### First-party clock and filesystem effects, provisional

`ReadSystemTime` (output `std::time::SystemTime`, infallible) with
`SystemClockDriver` and `bind_system_time()`. Behind the opt-in `fs` feature:
`AppendToFile`, `ReplaceFile`, `ReadFile`, and `RemoveFile`, one `FsError`
naming operation, root-relative path, and kind, and `FsDriver::new(root)`,
which rejects absolute paths and `..` before any I/O, creates missing parent
directories on writes, and performs replace as temp sibling, `sync_all`,
rename. `bind_fs(root)` binds only the effects the Program declares. These
are provisional: the filesystem design note's open questions (ordering,
Owned versus Shared handles, lazy versus eager root) remain open, and the
module may change shape without a further ADR until it is declared stable.

## Alternatives considered

- **`Model = Self`**, the Elm shape. Rejected: it moves the read-only-wiring
  guarantee from the compiler to convention, makes controlled `state()`
  snapshots include capability fields, and flattens enum-shaped Models into a
  struct that holds config too. It would be the natural spelling of an owned
  transition `update(self, msg) -> (Self, Command)`, so the two questions are
  tied; the owned signature stays in the follow-ups.
- **`type Config = Self`** as the standing idiom. Permitted by the trait but
  not used in the repository; every implementor, including rustdoc examples,
  splits kind from config so the public face shows one shape.
- **A `drivers!` macro** generating the `Bindings` impl. Rejected: a macro
  nobody wants to write, and it still needed `effect`/`source` keywords.
- **Inferring binding kind from the descriptor trait.** Blanket impls over
  `EffectDescriptor` and `SourceDescriptor` overlap; coherence rejects them.
  Putting the kind on the marker type (`Effect<D>` versus `Source<S>`) is
  what makes the trait parameter differ and the impls coherent.
- **A named-field driver struct.** Deferred, not rejected. Positional tuples
  lose field names at the host's call site; past five or six requirements
  that will hurt, which is also a signal the Assembly is too large. A
  `Requires`-level grouping that one driver satisfies (an `FsDriver` for
  several filesystem effects) is the expected extension.
- **Recording control closures in `Installed` during `install`.** Unneeded
  once `Requires` is static; the type-level list also gives both profiles a
  consistency check against what `install` declared.

## Consequences

- No change to transition purity, per-Component serialization, causal
  ordering, or controlled determinism. ADR-0002, 0003, 0004, and 0008 are
  unaffected; ADR-0008's two-stage validation is what makes a `Requires` list
  that disagrees with `install` fail `build()` on both profiles.
- The "observationally read-only configuration" conformance obligation moves
  from `&self` to `&Config` unchanged.
- Cost ledger additions: drivers are concrete at the call site and boxed once
  per binding inside `and_drivers`; Port bindings carry a boxed conversion
  (one allocation per binding, one indirect call per delivered message);
  `Requires` is positional.
- Every `Component` implementor changes signature. Mechanical; the compiler
  names each site.

## Evidence

`tests/assemblies.rs`; the `Assembly` and `VendorsDrivers` rustdoc with two
compile-fail cases; `tests/fs_effects.rs` (controlled intents by relative
path, live walk in a temp root, `OutsideRoot`, declared-only and duplicate
binding); the `connect_with` doctest; `examples/pulse` and `examples/url` as
worked examples of the kind/config split.

## Rollback

Revert the implementing commits. No persisted state or wire format is
involved. Applications assemble by hand and bind drivers directly, as before.
