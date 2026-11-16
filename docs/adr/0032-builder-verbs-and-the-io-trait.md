# ADR-0032: Builder Methods Are Verbs, and the Capability Trait Is `Io`

- Status: Accepted.
- Date: November 15, 2026
- Deciders: Samara maintainers
- Amends: [ADR-0029](0029-capabilities-are-named-values-a-component-owns.md)
  (the trait and derive are named `Io`),
  [ADR-0030](0030-registration-returns-its-handle.md) (the builder method
  names; its noun rule is withdrawn), and
  [ADR-0031](0031-connections-configuration-and-context.md) (the same
  names; two of its open questions are closed)
- Scope: `ProgramBuilder::add_component`, `configure_component`,
  `add_assembly`, `add_port`, `ComponentBuilder::add`, the `Io` trait,
  `#[derive(Io)]`

## Context

Two sets of names did not line up.

ADR-0030 made the builder methods that create something nouns
(`component`, `assembly`, `port`) and gave the detailed form the name
`component_builder`, ending in `.register(..)`. In use, a builder with a
method called `component_builder` and a terminal `build()` repeated itself,
and a bare noun did not say that something was being added:

```rust,ignore
let mut builder = Program::builder();
let monitor = builder.component_builder::<Monitor>().name("monitor").register(connections);
let program = builder.build()?;
```

After ADR-0031 a Component had `type Connections: Connections` beside
`type Io: Capabilities`. One associated type shared its name with its
trait and derive; the other did not.

## Decision

```rust,ignore
#[derive(Clone, Io)]
struct MonitorIo { input: Source<StdinLines>, output: Effect<PrintStdout> }

#[derive(Connections)]
struct MonitorConnections { journal: Connector<JournalMessage> }

let mut builder = Program::builder();
let audit = builder.add_port::<AuditMessage>(PortId::new("audit"));
let journal = builder.add_assembly::<Journal>(());
let monitor = builder.add_component::<Monitor>(connections);
let probe = builder
    .configure_component::<Probe>()
    .name("probe")
    .configuration(settings)
    .add(connections);
let program = builder.build()?;
```

- Builder methods are verbs: `add_port`, `add_component`, `add_assembly`,
  `connect`, `build`. `configure_component::<C>()` starts the detailed
  form and ends with `.add(connections)`, the same verb as the short form.
- The trait a `Component::Io` type implements is `Io`, with
  `#[derive(Io)]`. "Capability" remains the word for one value
  (`Effect<D>`, `Source<S>`, `File<P>`); an `Io` is the set a Component
  owns. `Io::detached()` replaces `Capabilities::detached()`.
- `Assembly::install`, `Declaring`, `DeclaredCapability`, and
  `CapabilityKind` keep their names.

A struct may share its name with the trait it derives. With
`use samara::prelude::*` a local `struct Io` or `struct Connections`
shadows the glob-imported trait and the derive still resolves. With an
explicit `use samara::Io` the two collide; derive by path instead:
`#[derive(Clone, samara::Io)] struct Io { .. }`.

Two questions ADR-0031 left open are closed:

- `init` keeps receiving the configuration by reference. Taking it by
  value would leave `update` unable to read settings and would leave
  nothing to re-initialize a Component from. Values derived from the
  configuration go in the Model.
- There is no setup step between `init` and the first `update`. The
  startup `Command` in `Init` is that step.

Faulting on a send to an undeclared address stays open. It is wanted, and
needs a declared opening for dynamic topology such as subscribers; see
`docs/notes/framework-follow-ups.md`.

## Alternatives considered

- **Rename the associated type to `Capabilities`** instead. The names
  would match, but `cx.capabilities.output` and `monitor.capabilities.file`
  are the long spellings ADR-0029 moved away from.
- **Keep `.register(..)` as the detailed form's terminal.** Three verbs
  (`add`, `configure`, `register`) for one act.
- **`Deref` from `Context` to its connections**, so `cx.journal` reaches a
  connection. It favours one of three fields for no stated reason, and a
  connection named `io` or `configuration` would be shadowed by the field.
- **`AsRef` from `Context` to each field.** The impls for `Io` and
  `Connections` conflict, since both are often `()`.
- **`Addressable` for a `Context` with one connection.** Works only until
  the Component gains a second connection, and hides the target.

## Consequences

- Every registration and every `Capabilities` bound or derive is
  respelled. No behavior changes: identity, ordering, and controlled
  determinism are untouched.
- A file that imports `samara::Io` explicitly and also uses `std::io`
  types is unaffected; the names differ by case.

## Evidence

The full test suite, including doc examples, under the new names.
`examples/pulse` derives by path for structs named `Io` and
`Connections`.

## Rollback

Revert the implementing commit.
