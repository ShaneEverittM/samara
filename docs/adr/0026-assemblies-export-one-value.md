# ADR-0026: Assemblies Export One Value

- Status: Accepted. Amended by
  [ADR-0029](0029-capabilities-are-named-values-a-component-owns.md):
  `install` takes no capabilities and returns `(Exports, Io)`, where `Io` is
  its members' capabilities. `Exports` is unchanged. Supersedes the `Accepts` associated type and the tuple
  result of [ADR-0018](0018-assemblies-export-a-connector.md). ADR-0018's
  rule that Assemblies declare no Ports continues as guidance.
- Date: November 9, 2026
- Deciders: Samara maintainers
- Scope: The shape of `Assembly::install`'s result; removal of
  `Assembly::Accepts`; when `InstalledAssembly::connector` exists

## Context

ADR-0018 made one Connector a required part of every Assembly, named by
`Accepts`, and left `Exports` for everything else. Living with it showed
that the privileged door buys convention only: every call site in the
repository reaches it through `.connector()`, which is one hop different
from reading it out of `Exports`. It also has two costs.

- **Peers get ranked.** An Assembly whose doors lead to different internal
  Components (an ingest side accepting samples and a read side accepting
  queries) has to call one of them the address and the other an extra.
- **An Assembly nobody sends to has a wart.** ADR-0018 has it name its
  facade Component's Message as `Accepts` and publish no constructors.

A second trait for the one-door case was considered. It cannot name its
Component, because an associated type in a public impl must be public
(E0446), so it could only promise one door, which the type of `Exports`
already says.

## Decision

```rust,ignore
pub trait Assembly {
    type Config;
    type Exports;
    type Requires: Requirements;
    fn install<R: Requirements>(
        program: &mut ProgramBuilder<R>,
        config: Self::Config,
        capabilities: Capabilities<Self::Requires>,
    ) -> Result<Self::Exports, ProgramBuildError>;
}

installed.exports()        // &A::Exports, always

// Only when A::Exports is a Connector<M>:
installed.connector()      // &Connector<M>
impl Addressable for InstalledAssembly<A> { type Message = M; }
```

The three shapes an Assembly takes:

```rust,ignore
type Exports = Connector<JournalMessage>;   // one door: journal.connector()
type Exports = TelemetryDoors;              // peers: telemetry.exports().queries
type Exports = ();                          // nobody sends to it
```

An Assembly with one door is written and used as it was under ADR-0018,
less one associated type and a tuple. `.connector()` keeps the spelling it
shares with `ComponentRef`, so a host wires a Component and a one-door
Assembly the same way.

## Alternatives considered

- **Keep ADR-0018.** Uniform location for the door, at the two costs above.
- **Two traits**, one for a single-door Assembly and one for the general
  case. More surface for what a conditional method provides, and the
  single-door trait could not enforce a single Component.
- **A trait for "has a primary Connector"** implemented by `Exports`
  structs, so a multi-door Assembly could still nominate one. This is the
  ranking the change removes; an author who wants it can name a field.

## Consequences

- `install` returns `Exports` alone; every `Assembly` impl loses `Accepts`.
  Host call sites of `.connector()` do not change.
- A multi-door Assembly names its doors as fields, and callers read them
  through `exports()`. There is no fixed name for them.
- ADR-0018 claimed a Port could not be exported. That was already untrue of
  `Exports`, and nothing structural prevents it now either. The direction
  ADR-0014 decided (the consumer owns the Port, the provider hands out a
  Connector) is guidance, and `.connector()` existing only for a Connector
  export is the nudge toward it.
- No change to transition purity, serialization, ordering, determinism, or
  binding validation.

## Evidence

`tests/assemblies.rs`: a struct export carrying a Connector and a
`ComponentRef`; an Assembly with two doors onto two Components, each
reached through its own field; an Assembly exporting `()`.
`tests/connectors.rs` and `examples/pulse`: single-door Assemblies used
through `.connector()` and as `Addressable`. The `connector` rustdoc
carries the compile-fail case for an Assembly whose export is not a
Connector.

## Rollback

Revert the implementing commit. `Accepts` and the tuple result return.
