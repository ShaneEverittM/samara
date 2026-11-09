# ADR-0018: Assemblies Export a Connector

- Status: Accepted. Amends the `Assembly` trait of ADR-0013.
- Date: November 9, 2026
- Deciders: Samara maintainers
- Scope: The shape of `Assembly::install`'s result; `Assembly::Accepts`;
  `InstalledAssembly::connector`; the rule that Assemblies declare no Ports

## Context

Under ADR-0013 an Assembly returned a single `Exports` value and the
repository's Assemblies disagreed about what to put in it: two exported a
`Connector`, two exported a `Port` of their own message type, one exported a
struct holding a Port and a `ComponentRef`. The Port-exporting shape is the
one ADR-0014 found points the dependency backwards, and it is also the source
of the open identity-scoping question in the assembly design note, because an
Assembly that makes a Port must name it, and two installs of the same
Assembly collide on the name.

Every Assembly has at least one Component, and every Component has a Message
type, so a Connector is always available. The Elm parallel is exact: every
module has a `Msg`, and a module nobody sends to simply exports no
constructors for it.

## Decision

```rust,ignore
pub trait Assembly {
    type Config;
    type Accepts: Send + 'static;          // the Connector's message type: the public vocabulary
    type Exports;                          // anything else, often ()
    type Requires: Requirements;
    fn install(program: &mut ProgramBuilder, config: Self::Config)
        -> (Connector<Self::Accepts>, Self::Exports);
}

installed.connector()      // &Connector<A::Accepts>
installed.exports()        // &A::Exports
impl Addressable for InstalledAssembly<A> { type Message = A::Accepts; }
```

An Assembly's address is a required part of the trait. The host cuts and
names its own Ports and connects the Assembly's Connector to them, or holds
the Connector in a Config and addresses it directly; an Assembly declares no
Ports. `Exports` carries whatever else a caller may need, such as a
`ComponentRef` for controlled tests, and is `()` when there is nothing. An
Assembly nobody sends to names its facade Component's Message as `Accepts`
and publishes no constructors.

The associated type is called `Accepts` because it is the message type the
Connector accepts, said from the host's side; `Message` would have read as
the Assembly's own message, which it is not.

## Alternatives considered

- **Encourage a Connector by convention** in guidance while keeping one
  `Exports`. Rejected: guidance that is only guidance does not survive a
  second author, and the Port-exporting shape was already in the repository.
- **Make `Exports` optional** through an associated-type default. Not
  available on stable Rust; `type Exports = ();` is one line and says
  "nothing else".
- **A separate `Exposes<M>` trait** so a struct export could also be an
  address. Rejected as two ways to say one thing.

## Consequences

- Exporting a Port from an Assembly is impossible rather than discouraged,
  so ADR-0014's dependency direction is structural.
- The identity-scoping question shrinks to `ComponentId`: Assemblies no
  longer name Ports.
- `InstalledAssembly<A>` is `Addressable`, so a host can send to an
  installed Assembly without unwrapping.
- One extra associated type and a tuple return on every `Assembly` impl.
- No change to transition purity, serialization, ordering, determinism, or
  binding validation.

## Evidence

`tests/assemblies.rs` (the host names its own Port and connects the
Assembly's Connector; the `ComponentRef` for controlled inspection rides in
`Exports`), `tests/connectors.rs` (two library Assemblies with
`Exports = ()`), `examples/pulse` (the journal exports a Connector and
declares no Port), and the `Assembly` and `VendorsDrivers` rustdoc with the
two compile-fail cases.

## Rollback

Revert the implementing commit. Assemblies return a single `Exports` again.
