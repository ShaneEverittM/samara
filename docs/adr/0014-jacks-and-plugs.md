# ADR-0014: Jacks and Plugs

- Status: Accepted
- Date: November 8, 2026
- Deciders: Samara maintainers
- Scope: The two ways a Component is exposed to others; the rename of `Port`
  to `Jack`; the new `Plug<M>` address and `Pluggable` trait

## Context

A library Assembly built under ADR-0013 exported a `Port` of its own
Protocol. Every application Component that used the library then held the
library's Protocol type, and plugging the library in meant implementing the
library's trait inside the application. Adding a dependency should not point
the dependency arrow backwards.

Working through the alternative showed that `Protocol` and `Port` were doing
three jobs: exposing a Component's public API, hiding its private Message
variants, and typing request and reply pairs. Elm does the first two with one
mechanism, exporting `Msg` as an opaque type and publishing constructor
functions for the callable subset, and never needed the third because its
queries are function calls on a shared Model. Rust cannot give enum variants
different visibilities, which is why the wrapper-variant ceremony exists; a
public newtype over a private enum is the faithful translation.

Once a provider-owned address existed beside the consumer-owned one, "Port"
was vague: it did not say which side owned it, and it collides with Elm's
JavaScript-interop ports for exactly the readers Samara most wants.

## Decision

### Two ways a Component is exposed, named by who owns the message type

A **Jack** is consumer-owned. The application declares a Protocol in its own
vocabulary and cuts a named `Jack<P>` that a provider is plugged into at
assembly. It is the tool for abstracting part of one's own application:
modularity, substitution, and test seams.

A **Plug** is provider-owned. `Plug<M>` is a `ComponentRef<C>` erased to the
Message type it accepts, carrying the pure conversion from `M` into
`C::Message` and the type identities a Jack binding validates. A Component or
Assembly hands it out to expose itself without naming its kind.

```rust,ignore
pub struct Plug<M> { /* id, program, type ids, convert: Arc<dyn Fn(M) -> Box<dyn Any + Send>> */ }
impl<C: Component> ComponentRef<C> {
    pub fn plug<M>(&self) -> Plug<M> where C::Message: From<M>;
    pub fn plug_with<M, Map>(&self, map: Map) -> Plug<M> where Map: Fn(M) -> C::Message;
}
pub trait Pluggable { type Message; fn to_plug(&self) -> Plug<Self::Message>; }
impl<C: Component> Pluggable for ComponentRef<C> {}
impl<M> Pluggable for Plug<M> {}
```

`Plug` is inert logical wiring like `ComponentRef` and `Jack`: it grants no
delivery capability. Its `Debug` output names the Message type it accepts and
not the Component.

### Jack binding accepts any `Pluggable`

`ProgramBuilder::plug_in(&jack, &provider)` and `plug_in_with(&jack,
&provider, map)` take `&impl Pluggable`. A `ComponentRef` is plugged in as
before; a `Plug` exported by a library is plugged in with the coupling from
the application's vocabulary to the library's written once at that site.
Internally a Jack binding is stored as the Jack plus a `Plug<P::Message>`, so
both forms share one representation and one boxed conversion.

### Rename

| Before | After |
| --- | --- |
| `Port<P>`, `PortId`, `PortHandle<P>` | `Jack<P>`, `JackId`, `JackHandle<P>` |
| `ProgramBuilder::port` | `ProgramBuilder::jack` |
| `bind_port`, `bind_port_with` | `plug_in`, `plug_in_with` |
| `LiveRuntime::port_handle` | `LiveRuntime::jack_handle` |
| `ProgramBuildError::{DuplicatePort, PortBindingCount, ForeignPort}` | `{DuplicateJack, JackBindingCount, ForeignJack}` |

ADRs 0001 to 0012 keep the old word as historical records; the glossary
carries the mapping. `Protocol`, `protocol!`, `Notification`, `Request`, and
`ReplyTo` are unchanged.

### What a library needs no Protocol for

A library may make its `Component::Message` a public newtype over a private
enum with public constructor functions, export `component(..).to_plug()`, and
write no `protocol!`. Hiding is then the module system's job, as in Elm.
`protocol!` stays the monolith's tool for typed request and reply pairs
within one application.

## Alternatives considered

- **Keep `Port`.** Correct hexagonal usage and no churn, but vague beside a
  second address and wrong for Elm readers. Rejected once the pair was needed.
- **`Address<M>`** for the provider-owned side, after Elm 0.16's
  `Signal.Address` and `forwardTo`. Accurate and well-sourced, but it pairs
  with nothing; `Jack` and `Plug` describe the two sides of one connection.
- **`Recipient<M>`**, kameo's word. Names a role rather than the thing held.
- **`Socket<P>` and `Plug<M>`.** The most literal pair, but `Socket` is
  unusable in a Tokio library.
- **`Inlet<M>` with `Port<P>`.** Same metaphor family, no collision, but it
  leaves `Port` in place.
- **A derive macro or a `dyn Component`.** Erasure by trait object was ruled
  out in ADR-0013; a macro generating `From` impls would not fix the direction
  of the dependency, which was the actual problem.

## Consequences

- No change to transition purity, per-Component serialization, causal
  ordering, or controlled determinism. Jack binding validation (ADR-0008) is
  unchanged; a Plug from another Program fails `build()` as a foreign
  provider.
- A Plug conversion composes with the Jack binding's conversion: one boxed
  closure per binding, one indirect call per delivered message, already on the
  cost ledger.
- `Plug<M>` is a binding target only. `Command::send` over any `Pluggable`, a
  `Command::request_to` that mints the `ReplyTo` against a Plug from a
  constructor function, and a public `Plug::with_input` are recorded in the
  framework follow-ups; the request form touches both runtimes and needs an
  amendment here.
- Every public use of the old names changes. Mechanical; the compiler names
  each site. Landed before 0.1, so no released surface is affected.

## Evidence

`tests/plugs.rs`: a library module exports `Plug<JournalMessage>` while its
Component is private; the application declares its own `Audit` Protocol and
couples with one function; notify, request, and reply travel through the
coupling under controlled execution; a Plug is inert and names only what it
accepts; a foreign Plug fails `build()`. The `Plug` rustdoc example. The
existing Jack suites (`tests/external_jack_ingress.rs`, request and
notification tests) pass unchanged under the rename.

## Rollback

Revert the implementing commits. No persisted state or wire format is
involved. A library that exported a Plug exports a Jack of its own Protocol
again, and applications implement that Protocol's conversion themselves.
