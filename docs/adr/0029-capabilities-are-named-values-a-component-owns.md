# ADR-0029: Capabilities Are Named Values a Component Owns

- Status: Accepted. [ADR-0030](0030-registration-returns-its-handle.md)
  then removed `finish()` and the `Result` from registration; the snippets
  below use that form.
- Date: November 15, 2026
- Deciders: Samara maintainers
- Completes: [ADR-0028](0028-a-capability-belongs-to-one-component.md)
- Amends: [ADR-0019](0019-typed-program-requirements.md) (`Requires`, the
  sealed `Requirements` trait, `Program<R>`),
  [ADR-0022](0022-file-resource-capabilities.md) (one `File` type; no
  narrowing),
  [ADR-0026](0026-assemblies-export-one-value.md) (what `install` returns),
  and [ADR-0027](0027-drivers-are-supplied-per-capability.md) (the supply
  spelling)
- Scope: `Component::Io`; the `Capabilities` trait, `Declaring`, and
  `#[derive(Capabilities)]`; the merged `Effect`, `Source`, and `File`
  types; `Assembly::Io` and `install`; `RuntimeSetup::supply`;
  `InstalledComponent`; capability names in the manifest and in errors

## Context

ADR-0028 decided that registration creates a Component's capabilities and
that nothing is shared, and landed the host-facing half beside the old API.
Finishing it meant replacing `type Requires`, and that raised the question
ADR-0028 left open: what tells two capabilities of the same type apart
inside one Component. Position was the only answer. A Component with a
journal file and a snapshot file had `(File<ReadAppend>, File<ReadAppend>)`,
could destructure them backwards, and could be handed their Drivers in
either order.

Three carriers for identity were considered: position, a type-level tag,
or a name. A tag makes a swapped driver tuple a compile error but does
nothing for a Component that misnames its own variables. A name protects
both places as long as it appears on both sides of each pairing, and it is
what a reader wants in `update` and in the host anyway.

Names were out of reach while a requirement list had to be a type the
framework could walk. They became cheap once registration creates the
capabilities: a Component can build its own struct in an ordinary function,
one line per field.

## Decision

### A Component names its capabilities as `type Io`

```rust,ignore
#[derive(Clone, Capabilities)]
pub struct JournalIo {
    pub file: File<ReadAppend>,
    pub clock: Effect<ReadSystemTime>,
}

impl Component for Recorder {
    type Io = JournalIo;
    fn update(config: &(), io: &JournalIo, model: &mut Model, message: Message) -> Command<Message> {
        // Effect::perform(&io.clock, ReadSystemTime), File::append_line(&io.file, ..)
    }
}
```

`Io` replaces `Requires`. It must implement `Capabilities`, whose one
required function creates a fresh value:

```rust,ignore
pub trait Capabilities: Clone + Send + 'static {
    fn declare(new: &mut Declaring<'_>) -> Self;
    fn detached() -> Self { .. }
}
```

- The library implements it for `Effect<D>`, `Source<S>`, `File<P>`, `()`,
  and tuples, so a small Component or a test writes
  `type Io = (Effect<Tag>,)` and nothing else.
- `#[derive(Capabilities)]` implements it for a struct with named fields.
  The derive lives in a new `samara-derive` crate, re-exported from
  `samara`. A hand-written impl is the same three lines:
  `Self { file: new.field("file"), clock: new.field("clock") }`.
- A capability can only be made through `Declaring`, which only the library
  constructs.
- `detached()` makes capabilities that belong to no Program, for calling
  `update` directly in a unit test and inspecting the Command it returns.

### One set of types

The type-level markers and the value types are merged. `Effect<D>`,
`Source<S>`, and `File<P>` are the capability values; `EffectCapability`,
`SourceCapability`, `FileCapability`, the sealed `Requirements` trait, and
the `Capabilities<R>` alias are removed. `Effect::perform(&effect, ..)` and
`File::read(&file)` read as before. The `File` narrowing methods are
removed with sharing.

### The host names a capability and supplies its Driver

```rust,ignore
let mut builder = Program::builder();
let journal = builder.install::<Journal>(());
let monitor = builder.component::<Monitor>(journal.connector());

Runtime::builder(builder.build()?)
    .supply(&journal.io.file, FileDriver::new("./data/pulse.journal"))
    .supply(&journal.io.clock, SystemClockDriver)
    .supply(&monitor.io.input, StdinDriver)
    .supply(&monitor.io.output, StdoutDriver::new())
    .build()?;
```

- `Program::builder()` takes no type; `Program`, `ProgramBuilder`,
  `RuntimeSetup`, and `ControlledRuntimeSetup` have no type parameter.
- Registration returns `InstalledComponent<C>` for every Component. Its `io`
  field is the value the Component uses. It derefs to `ComponentRef<C>`,
  converts into one, and is an address.
- `supply(&capability, driver)` replaces ADR-0027's
  `with(&capability).supplied_by(driver)`. With names, the pair on one line
  reads as a sentence. It takes one capability or a tuple of them; a Driver
  of the wrong type does not compile.
- Every capability is bound to its own Driver, including plain effects and
  type-wide sources. Two Components that read the clock are each supplied
  one. Whether they share a real resource is the host's choice: it can
  supply clones of one Driver.

### An Assembly returns its members' capabilities

```rust,ignore
impl Assembly for Journal {
    type Config = ();
    type Exports = Connector<JournalMessage>;
    type Io = JournalIo;
    fn install(program: &mut ProgramBuilder, _: ()) -> (Self::Exports, Self::Io) {
        let recorder = program.component::<Recorder>(());
        (recorder.accepting(Message::Journal), recorder.io.clone())
    }
}
```

`install` no longer receives capabilities. `Assembly::Io` has no bound: it
is one member's `Io`, a struct of several, or `()`. `VendorsDrivers` becomes
`fn supply(setup, io, driver_config) -> RuntimeSetup`.

### Ownership is enforced, and names reach diagnostics

- A Component that uses a capability created for another Component faults:
  "belongs to another Component". This is checked wherever a capability
  from another Program was already checked.
- A capability left without a Driver fails `build()` naming its owner, its
  field, and its descriptor type:
  `Effect capability Recorder.file (FileAppend) has 0 live bindings`.
- `Program::manifest()` rows carry the owner and the field name.

## Alternatives considered

- **A type-level tag**, such as `File<ReadAppend, JournalFile>`, with the
  Driver carrying the tag. Needs no macro and keeps whole-tuple supply safe,
  but costs a marker type per resource and does not help inside `update`.
  Not needed once names exist on both sides.
- **Names and tags together.** Both put identity on both sides of the
  pairing; stacking them adds ceremony for a mistake names already make
  hard to commit.
- **A derive as the only way to implement the trait.** Rejected so that the
  trait stays one small function a person can write and the derive stays
  sugar over it.
- **A generated drivers struct**, so a named group could be supplied whole
  with a missing field as a compile error. Deferred; field-by-field supply
  is being lived with first.
- **A different handle type when `Io = ()`.** The interim returned a plain
  `ComponentRef` in that case. Uniform is simpler to explain and does not
  need hidden plumbing in every `Capabilities` impl.

## Consequences

- An application names no requirement type and destructures no tuple.
  Capabilities have names in `update`, at the host, in the manifest, and in
  startup errors.
- The workspace gains a proc-macro crate with `syn` and `quote` as build
  dependencies.
- A named struct is supplied field by field. Tuples can still be supplied
  whole, which is as positional as it ever was; that is acceptable for small
  Components and tests.
- Capabilities may no longer be kept in `Config`. A Component uses the `io`
  it is passed.
- Tests whose subject was sharing, narrowing, up-front declaration, or
  rejection of a foreign capability at registration no longer have a
  subject. Each was re-pointed at the new rule or marked ignored with the
  reason; none was silently deleted.
- Controlled execution still controls type-wide sources per descriptor
  type, so a controlled test cannot yet address one of two same-typed
  type-wide sources separately. Effects, files, and stream sources are
  selected per capability.
- `control_all()` no longer distinguishes anything: every capability is
  controlled. It remains so existing tests read the same;
  `ControlledRuntime::builder(program).build()` does the same thing.
- No change to transition purity, per-Component serialization, causal
  ordering, or controlled determinism.

## Not included

Three pieces of the imagined host shape are independent of this decision
and are left for follow-ups: `run_until(signal)` in place of spawn, await,
and drain; registering a Component by value when its `Config` is itself;
and registration that does not return a `Result`.

## Evidence

`tests/owned_capabilities.rs`: per-Component Drivers for the same effect, a
missing Driver named by owner, controlled execution over created
capabilities, the ownership fault, a derived named `Io` with field names in
the manifest and in the missing-Driver error. `tests/capability_binding.rs`:
Drivers follow the capability for two same-typed files; missing, duplicate,
and foreign cases. `tests/assemblies.rs`: the new `install` and
`VendorsDrivers::supply`. `examples/pulse`: a derived `JournalIo` and
`MonitorIo`, field-by-field supply, and the journal's pinned manifest.

## Rollback

Revert the implementing commit. `Requires`, the up-front builder, and
`with(..).supplied_by(..)` return together.
