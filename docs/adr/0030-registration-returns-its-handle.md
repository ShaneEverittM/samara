# ADR-0030: Registration Returns Its Handle

- Status: Accepted.
  [ADR-0031](0031-connections-configuration-and-context.md) changed what
  registration is given: `component::<C>(connections)`, with the
  configuration defaulted or set by `.configuration(..)` on the builder.
- Date: November 15, 2026
- Deciders: Samara maintainers
- Amends: [ADR-0021](0021-implicit-identities-and-diagnostic-names.md)
  (where the diagnostic name is set),
  [ADR-0026](0026-assemblies-export-one-value.md) (what
  `InstalledAssembly::connector` returns), and
  [ADR-0029](0029-capabilities-are-named-values-a-component-owns.md) (the
  registration and `install` signatures)
- Scope: `ProgramBuilder::component`, `ProgramBuilder::component_builder`,
  `ProgramBuilder::assembly` (formerly `install`), `Assembly::install`,
  `InstalledAssembly::connector`

## Context

Registration was a staged builder ending in `finish()`, which returned a
`Result`:

```rust,ignore
let journal = builder.install::<Journal>(()).finish()?;
let monitor = builder.component::<Monitor>(journal.connector().clone()).finish()?;
```

After ADR-0029 both the stage and the `Result` were vestigial. The stage
existed so capabilities could be supplied before `finish()`, and
registration no longer accepts capabilities; on an Assembly it had no
optional step at all, and on a Component only `.name(..)`. The only way
registering a Component could fail was being handed a capability from
another Program, which can no longer be expressed, so every `finish()?`
returned an `Ok` that could never be an `Err`.

The `.clone()` came from an inconsistency: `ComponentRef::connector()`
returns an owned `Connector`, and `InstalledAssembly::connector()` returned
a reference.

## Decision

```rust,ignore
let journal = builder.assembly::<Journal>(());
let monitor = builder.component::<Monitor>(journal.connector());

// When the host wants to say more about an instance:
let probe = builder.component_builder::<Probe>().name("probe").register(config);
```

- `component::<C>(config)` registers and returns `InstalledComponent<C>`.
- `component_builder::<C>()` is the second entry point, for per-instance
  options. Today the only option is the diagnostic `.name(..)`; it ends with
  `.register(config)`. This is the split `tokio::spawn` and
  `tokio::task::Builder` make: one operation at two levels of detail.
- `assembly::<A>(config)` installs and returns `InstalledAssembly<A>`. The
  builder method is renamed from `install` so that the methods that create
  something and return its handle are all nouns (`port`, `component`,
  `assembly`) and the methods that act on what exists are verbs (`connect`,
  `build`). `Assembly::install`, the function an author implements, keeps
  its verb.
- `Assembly::install` returns `(Exports, Io)`, not a `Result`.
- `InstalledAssembly::connector()` returns an owned `Connector`, as
  `ComponentRef::connector()` does.

`ProgramBuildError` remains for what can still fail: `connect` and
`build`.

## Alternatives considered

- **`component_named::<C>(name, config)`.** Does one thing forever. A
  builder has somewhere for later per-instance options to go.
- **A `name(config)` method on `Component`**, defaulting to the type name.
  Puts the name with the author and removes the second entry point, but the
  host can then label an instance only through its config. It can still be
  added as the default beneath the host's override if type-path names in
  the manifest prove a nuisance.
- **`.build(config)` as the terminal verb.** It would sit beside
  `builder.build()`, which builds the Program.
- **`component::<C>(config: impl Into<C::Config>)`** to avoid the clone.
  Only helps when the Connector is the whole config, hides a conversion, and
  turns a wrong argument into a trait-bound error.
- **A builder for Assemblies.** It would have no options yet.
- **A verb pair, `register::<C>` and `install::<A>`.** Matches the prose
  ("register a Component", "install an Assembly") but `register::<Monitor>`
  does not say what kind of thing it registers, and it renames the far more
  common call.

## Consequences

- Assembly code reads as plain statements with no `?`.
- A `ComponentBuilder` that is dropped without `register` registers
  nothing, as a dropped staged registration did before.
- Where an Assembly's Connector is passed by reference, such as
  `builder.connect(&port, &journal.connector())`, the call site gains a `&`.
- If registration ever gains a real failure mode, the `Result` comes back
  on these signatures.
- No change to identity, ordering, or controlled determinism: registration
  order still determines `ComponentId` and startup order.

## Evidence

Every registration in the tests, examples, and doc examples uses the new
forms. `tests/identity.rs` keeps the test that a dropped, unregistered
builder leaves identities and the startup trace unchanged.

## Rollback

Revert the implementing commit.
