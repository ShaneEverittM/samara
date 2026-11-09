# ADR-0015: Ports, Connectors, and the Message Type

- Status: Accepted. Supersedes the naming and the `Protocol` trait of
  ADR-0014; the `Plug` semantics of ADR-0014 continue unchanged under the
  name `Connector`.
- Date: November 8, 2026
- Deciders: Samara maintainers
- Scope: The type parameter of a Port; removal of the `Protocol` trait; the
  names of the two exposure handles

## Context

ADR-0014 named the two ways a Component is exposed Jack and Plug and gave
the provider-owned side a `Plug<M>` typed by the message it accepts. Set
beside it, the consumer-owned side looked odd: `Jack<P>` was typed by a
`Protocol`, a unit type whose only content was `type Message`. The two
handles were meant to be the two ends of one connection, but one was typed
by what it carries and the other by a name for what it carries.

The Elm comparison that produced ADR-0014 also showed what `Protocol` was
not. A library written the Elm way, an opaque message type with constructor
functions, has a protocol in every meaningful sense and never touched the
trait. The glossary had already said as much by defining **Protocol** and
**Protocol message** with the same sentence. The trait was the
Component-kind pattern applied where there was nothing to be a kind of: a
Component kind names three associated types, a Protocol named one.

The words Jack and Plug were chosen from the audio workbench. Port and
Connector say the same thing in domain-neutral language, restore the
hexagonal term for the consumer-owned side, and make ADRs 0001 to 0012 read
consistently again.

## Decision

### A Port is typed by the message type it carries

```rust,ignore
pub struct Port<M>;                         // consumer-owned, filled at assembly
pub struct Connector<M>;                    // provider-owned address
pub trait Notification<M>: Send + 'static { fn into_message(self) -> M; }
pub trait Request<M>: Send + 'static { type Reply; fn into_message(self, reply_to: ReplyTo<Self::Reply>) -> M; }
pub struct RequestInvocation<M, R: Request<M>> { pub request: R, pub reply_to: ReplyTo<R::Reply> }
```

`Protocol` is removed. `Port<M>`, `PortHandle<M>`, `Notification<M>`, and
`Request<M>` are parameterised by the message type they carry or convert
into, with `M: Send + 'static`. `RequestInvocation` keeps its first
parameter because `R::Reply` must be pinned to one `Request<M>` impl. Port
identity is the message type plus the `PortId`, which is the same uniqueness
rule as before with the marker removed.

The `protocol!` macro becomes `messages!` and loses its marker clause:

```rust,ignore
messages! {
    pub enum AuditMessage {
        Changed { what: String, from: String, to: String },
        Tail(usize) -> Vec<String>,
    }
}
let audit = program.port::<AuditMessage>(PortId::new("audit"));
```

An Assembly that implemented `Assembly` on the protocol marker now declares
its own public marker struct, which was always what that type was.

### Two handles, two names

| ADR-0014 | This ADR | Owned by | Typed by |
| --- | --- | --- | --- |
| `Jack<P>` | `Port<M>` | consumer | the message type it accepts |
| `Plug<M>` | `Connector<M>` | provider | the message type it accepts |
| `Pluggable` | `Connectable` | | |
| `plug_in`, `plug_in_with` | `connect`, `connect_with` | | |
| `ComponentRef::plug`, `plug_with`, `to_plug` | `connector`, `connector_with`, `to_connector` | | |
| `JackId`, `JackHandle`, `jack_handle` | `PortId`, `PortHandle`, `port_handle` | | |

Everything else from ADR-0014 holds: `Command::send` over any
`Connectable`, `Command::request_to` and `request_to_timeout` minting the
`ReplyTo` against a Connector from a published constructor function,
`Connector::accepting`, and the trace shape.

The glossary story is now two sentences. A `Port<M>` is a consumer-owned,
named hole that accepts `M`, filled at assembly. A `Connector<M>` is a
provider-owned address that accepts `M`, used directly or connected to a
Port.

## Alternatives considered

- **Keep `Protocol` as a blanket-implemented marker.** It would have kept
  `Port<Audit>` short, but a trait every message type implements for free
  says nothing, and the two handles would still be typed differently.
- **Keep Jack and Plug.** Vivid and already landed, but the pair carried a
  metaphor rather than a meaning, and `Port` is the term a hexagonal reader
  already knows.
- **`Socket`, `Address`, `Inlet`.** Rejected in ADR-0014 for reasons that
  still apply.

## Consequences

- No change to transition purity, per-Component serialization, causal
  ordering, controlled determinism, or Port binding validation. Only names
  and one type parameter move.
- `Port<AuditMessage>` is longer than `Port<Audit>`. The message enum is the
  one type a consumer must know anyway.
- Hand-written ports no longer need a marker type: any `Send + 'static`
  type is a Port parameter, including `()`.
- The Elm `port` collision that ADR-0014 cited remains and is noted in the
  glossary.

## Evidence

Every existing Port, request, notification, and Connector suite passes
unchanged in behavior under the new names, including `tests/connectors.rs`
(formerly `plugs.rs`) and `tests/external_port_ingress.rs`. The `messages!`
unit tests cover unit, tuple, and struct-like entries as before.

## Rollback

Revert the implementing commit. No persisted state or wire format is
involved.
