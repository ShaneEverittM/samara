# ADR-0019: Typed Program Requirements

- Status: Accepted for implementation trial.
- Date: November 9, 2026
- Amends: ADR-0005, ADR-0008, ADR-0009, ADR-0013, and ADR-0018.

ADR-0021 amends Component registration to `component::<C>(config)` with an
optional diagnostic `.name(...)`; Component identity is issued by the builder.

## Decision

`Program::builder::<R>()` declares the complete requirement shape and returns
`(ProgramBuilder<R>, Capabilities<R>)`. `R` is `()`, `Effect<D>`, `Source<S>`,
or a nested tuple (up to eight positions per tuple). Capabilities are issued
once, carry Program provenance and exact identity, and can be cloned for
sharing. There is no public operation to add capabilities after this call.

Components declare `type Requires: Requirements`. Their immutable settings and
connections remain in `Config`; `init`, `update`, and `subscriptions` also
receive `&Capabilities<Self::Requires>`. Registration is staged:
`component::<C>(config).with_capabilities(caps).finish()?`. The final step
checks capability provenance before calling `init` or changing the graph.
Components requiring `()` may omit `with_capabilities(())`.

Assemblies receive their capability tuple from the host and forward subsets to
their internal Components. Installation uses
`install::<A>(config).with_capabilities(caps).finish()?` and returns a Connector
and exports through `InstalledAssembly`, which is now an
address/export container, not an outstanding runtime binding obligation.
Assemblies cannot expand the Program's requirements. Installation errors
propagate as `ProgramBuildError`.

Component registration and Assembly installation both end with `finish`,
including the shortcuts for empty requirements. Within Program assembly,
`build` is reserved for finalizing the complete Program and validating its graph.

`Program<R>` carries the binding contract into the live runtime builder.
`with_drivers(Drivers<R>)` supplies the complete tuple and makes `build`
available. The controlled builder's `control_all()` selects controlled
behavior for all declared boundaries and makes `build` available. Neither
profile executes work while assembling. The completed runtimes erase `R`.
No per-Component or per-Assembly installation token must be transported to a
runtime builder.

`with_drivers` is the only public operation that supplies live Drivers. The
returned `RuntimeBuilder` only validates and builds the runtime; it exposes no
incremental `bind_*` methods. `Drivers<R>` is a sealed marker whose binding
implementation is private, so its trait cannot bypass this phase boundary.
The internal registry retains duplicate, conflict and provenance validation.
First-party implementations are ordinary tuple values: `HttpDriver`,
`StdoutDriver`, `StderrDriver`, `SystemClockDriver`, `FsDriver`,
`DrivenSource<TcpDriver>`, `MpscReceiver`, and Unix `StdinDriver`.

An Assembly may implement `VendorsDrivers` with a separate `DriverConfig`
associated type. `drivers(config: Self::DriverConfig) -> Self::Drivers`
consumes live binding settings and returns the tuple supplied to `with_drivers`.
Use `DriverConfig = ()` when there are no settings. This config is independent
of `Assembly::Config`, is never installed in the Program, and is unnecessary
for controlled execution. A host may construct different driver tuples for
the same logical Program without changing its Components or descriptors.

For the Journal example, `JournalConfig.file` is the user-supplied relative
filename forwarded into the Recorder's immutable config. `JournalConfig::default()`
selects `JOURNAL_FILE`. The host supplies `FsDriver::new(root)` and
`SystemClockDriver` directly in `main`; the root is not part of logical
application wiring. Journal needs no vendored-driver wrapper for these
standard implementations. This does not change filesystem path validation or
make configuration imply any file I/O.

The requirement machinery is sealed: application implementations cannot
declare one type shape while omitting its capabilities or binding work.
Source requirements preserve both application descriptor and terminal type.
Exact channel receivers bind to the capability at their tuple position;
controlled channels retain exact capability selection. Built-in Layers still
run in both profiles. A type-wide source Driver is wrapped in `DrivenSource` and binds the terminal
descriptor. `SourceDescriptor::Terminal` names that type (`Self` for custom
terminal sources); built-in Layers propagate it. This avoids overlapping Rust
trait implementations between ordinary Drivers and exact receiver adapters.

Shared type-wide dependencies should occupy one position in the Program
requirements, with cloned capabilities passed to multiple Components. Exact
resources may occupy several positions of the same source type. Duplicate or
ambiguous type-wide bindings remain errors; tuple positions do not establish
event ordering or component ownership of a world resource.

## Invariants and failure modes

- The transition remains pure: immutable Config and capabilities, exclusively
  owned mutable Model, Message input, and typed Command output.
- Per-Component serialization, causal ordering, controlled traces, logical
  time, source reconciliation and shutdown behavior are unchanged.
- Missing/wrong capability tuples and missing/wrong live driver tuples fail
  compilation. No `build` is exposed on an unfinished binding stage.
- Foreign capability values fail component/assembly registration before init.
  Foreign values deliberately hidden in arbitrary Config/Model fields retain
  ADR-0008's initial and later runtime defenses.
- Duplicate identities, invalid connections, duplicate/ambiguous bindings,
  malformed source lowering and invalid startup work still fail synchronous
  validation. Type metadata does not prove value identity or transition purity.
- Capability declarations never drift from the typed inventory: the same
  sealed operation constructs both the inventory and the returned values.
- Driver settings affect only the vendored live implementations; supplying
  them does not alter the Program's capability requirements or start work.
- Dropping a registration builder adds no Component. Dropping an unbound
  runtime builder starts no work. Failed assembly should discard its enclosing
  ProgramBuilder, as installation may already have registered child Components.

## Evidence

`tests/typed_program.rs` covers capability sharing, callback delivery, delayed
effects, live/controlled construction, foreign registration before init, exact
source identities, and Assembly forwarding. `tests/closed_program_capabilities.rs`
retains composed-source and exact-stream execution evidence. Rustdoc compile-fail
contracts cover incomplete registration/binding and incorrect tuples. Existing
runtime, source, time and shutdown acceptance suites remain required.
Additional compile-fail contracts reject incremental live binding and direct
trait binding. Duplicate and conflicting driver tuples remain integration
tests; internal registry tests retain foreign exact-capability defenses.

The `pulse` example verifies separate filename and root configuration. A
Transition emits one `AppendToFile::line` containing `name: from -> to` and
discards its outcome without retrying. Recent-history requests issue `ReadFile`
and return up to the requested number of trailing lines from the returned
bytes, in file order. They have no ordering guarantee against concurrent
appends; an unterminated trailing line is included.

Recent replies carry `Result<Vec<String>, journal::Error>`. Under ADR-0020,
the effect mapper routes success or an `EffectFailure<FsError>` to a Message;
update maps error and cancellation reasons to the public Journal error
variants. Controlled tests cover tail boundaries, request correlation,
completion orders, domain mapping, and discarded append outcomes. Live tests
verify configured paths, existing-file reads, and missing-file errors.
Invalid UTF-8 and the application's broader error handling remain unfinished,
so `pulse` is not acceptance evidence for a complete application.

## Tradeoffs and rollback

The root requirement list is explicit. Nested tuples support grouping without
making the whole component graph part of the type. Registration does not
change the ProgramBuilder's type, preserving ordinary mutable graph assembly.
This breaks Component callbacks, Program construction, Assembly installation,
and runtime binding call sites. Exact resources require binding values that
retain their position's capability identity.

Rollback restores the previous APIs and their callers together. There is no
persisted state or wire-format change. Keep capability provenance and complete
profile validation when reverting the type-level convenience and enforcement.
