# ADR-0031: Connections, Configuration, and Context

- Status: Accepted.
- Date: November 15, 2026
- Deciders: Samara maintainers
- Amends: [ADR-0013](0013-component-kinds-assemblies-and-first-party-effects.md)
  (`Component::Config` is split in two),
  [ADR-0019](0019-typed-program-requirements.md) (the `Component`
  signature), [ADR-0029](0029-capabilities-are-named-values-a-component-owns.md)
  (the Component function signatures and what the manifest lists), and
  [ADR-0030](0030-registration-returns-its-handle.md) (what registration is
  given)
- Scope: `Component`, `Context`, `Connections`, `#[derive(Connections)]`,
  `ProgramBuilder::component`, `ComponentBuilder`, `ProgramBuilder::build`,
  `Program::manifest`

## Context

A Component had one `Config` type holding two unrelated things:

```rust,ignore
impl Component for Monitor {
    type Io = MonitorIo;
    type Config = MonitorConfig;   // { journal: Connector<JournalMessage>, interval: Duration }
    fn update(config: &Self::Config, io: &Self::Io, model: &mut Model, message: Message) -> ...
}
```

The two halves come from different people and mean different things:

- **Settings** (an interval, a threshold) come from the host or the
  deployment. They are plain data and mean the same in any Program.
- **Connections** (`Connector`, `Port`, `ComponentRef`) come from whoever
  assembles the Program. They mean something only inside that Program.

Because they shared one opaque type, Samara could not see either. After
ADR-0029 a Program could list everything it can reach in the world, but
not which Component can send to which: the wiring was buried in `Config`.
A Connector from another Program was found only when a Command using it
was interpreted.

The function signatures had also reached four parameters, and a third
associated value would have made five.

## Decision

A Component has three immutable inputs, each with one provenance, and its
functions receive them as one `Context`:

```rust,ignore
#[derive(Connections)]
struct MonitorConnections { journal: Connector<JournalMessage> }

#[derive(Default)]
struct MonitorSettings { interval: Duration }

impl Component for Monitor {
    type Io = MonitorIo;                       // the world; created by registration
    type Connections = MonitorConnections;     // other Components; supplied by the assembler
    type Configuration = MonitorSettings;      // plain settings; supplied by the host
    type Model = Model;
    type Message = Message;

    fn init(cx: &Context<Self>) -> Init<Model, Message>;
    fn update(cx: &Context<Self>, model: &mut Model, message: Message) -> Command<Message>;
    fn subscriptions(cx: &Context<Self>, model: &Model) -> Subscriptions<Message>;
    fn shutdown(cx: &Context<Self>, model: Model) -> Command<NoMessage>;
}
```

- `Context<C>` is a struct with three public fields: `io`,
  `connections`, and `configuration`. It is immutable for the Component's
  whole life. `Context::detached(connections, configuration)` makes one
  with detached capabilities for calling the functions directly in a unit
  test.
- `Connections` is a trait Samara can walk. It is implemented for `()`,
  `Connector<M>`, `Port<M>`, `ComponentRef<C>`, `Option` and `Vec` of
  those, tuples up to eight, and structs with named fields through
  `#[derive(Connections)]`. A field name labels the connection the way a
  field name labels a capability.
- `Configuration` is any `Send + 'static` type. Samara does not look
  inside it.
- Registration takes the connections. The configuration defaults, and is
  set through the builder when it should not:

  ```rust,ignore
  let monitor = builder.component::<Monitor>(journal.connector());

  let monitor = builder
      .component_builder::<Monitor>()
      .name("monitor")
      .configuration(MonitorSettings { interval })
      .register(journal.connector());
  ```

  `component` and a builder without `.configuration(..)` require
  `Configuration: Default`.
- `ProgramBuilder::build` checks every declared connection belongs to
  this builder and fails with `ProgramBuildError::ForeignConnection`
  otherwise.
- `Program::manifest()` returns a `Manifest` (formerly
  `CapabilityManifest`) holding `capabilities()` and `connections()`. Each
  connection row names the owner, the field, the message type, and the
  Component that will receive it; a connection through a Port is resolved
  to the Port's provider:

  ```text
  pulse::Monitor.input: source samara::StdinLines
  pulse::Monitor.output: effect samara::PrintStdout
  pulse::Monitor: connection pulse::journal::JournalMessage -> pulse::journal::Recorder
  ```

`Assembly::Config` is unchanged: it is the argument of a function, and the
Assembly decides which parts become its members' connections and which
their configuration.

## Invariants

- The transition is still pure. `Context` is shared, immutable input;
  nothing in it can be changed by `update`.
- No change to identity, ordering, causality, or controlled determinism.
  Listing connections happens at registration and is not observable by a
  running Program.
- The manifest lists declared connections. An address a Component holds
  some other way (in its `Configuration`, or received in a Message) is not
  listed and is not checked at `build`; using a foreign one still faults
  when the Command is interpreted, as before.

## Alternatives considered

- **Keep one `Config` and split it by convention.** Costs nothing, and
  gives Samara nothing to walk: no topology in the manifest and no build
  check.
- **Five parameters.** `update(configuration, connections, io, model,
  message)` keeps everything positional, but every Component pays for all
  five in every function, and most use one or two.
- **Two registration arguments**, `component::<C>(connections,
  configuration)`. Explicit, but most Components have neither, and
  `((), ())` at most call sites is noise.
- **Configuration as the positional argument and connections on the
  builder.** Registration is where the graph is assembled, so the graph
  edge is the argument that belongs in the short form.
- **Fault on any send to an undeclared address**, making the connection
  list complete the way capability ownership makes the capability list
  complete. Deferred: it changes what a Message may carry, and needs its
  own decision.

## Consequences

- Every Component implementation changes: two associated types replace
  one, and each function takes `cx` in place of `config` and `io`.
- A Component whose settings have no sensible default must be registered
  through `component_builder().configuration(..)`.
- `CapabilityManifest` is renamed `Manifest` and `entries()` becomes
  `capabilities()`. Pinned manifest text gains connection rows.
- A foreign address placed in `Connections` is now reported at `build`
  with the Component and field that hold it.
- `samara::Context` shares its name with `std::task::Context` and
  `anyhow::Context`; a file that uses both imports one explicitly.

## Open questions

- Whether `init` should receive the configuration by value, once, so a
  Component can compute or prepare from it, and whether a separate setup
  step should run between `init` and the first `update`.
- Whether undeclared addresses should fault (see alternatives).
- Whether `Assembly::Config` should be split the same way.

## Evidence

`tests/connections.rs` covers the manifest rows for a bare Connector, a
derived struct, a Port resolved to its provider, a `ComponentRef`, and
`Option`/`Vec`; the `ForeignConnection` build error; configuration
defaulting and the builder override; and `Context::detached` in a pure
test. Every Component in the tests, examples, and doc examples uses the
new signatures.

## Rollback

Revert the implementing commit.
