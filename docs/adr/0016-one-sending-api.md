# ADR-0016: One Sending API

- Status: Accepted
- Date: November 8, 2026
- Deciders: Samara maintainers
- Scope: Removal of `messages!`, `Notification`, `Request`,
  `RequestInvocation`, the Port-only `Command` constructors, and their
  `PortHandle` and inspector counterparts; `Addressable` as the one way a
  Command names its destination

## Context

ADR-0014 gave Connectors `Command::send` and `Command::request_to`, the
constructor form: a request is a value of the destination's message type
whose variant carries a `ReplyTo<R>`, built by a function the provider
published, and the reply type is fixed by that parameter. ADR-0015 typed
Ports by the same message type. That left two spellings of one request side
by side:

```rust,ignore
Command::request_with(audit, Tail(1), MonitorMessage::Tail)                         // trait form
Command::request_to(audit, |reply_to| AuditMessage::Tail(1, reply_to), MonitorMessage::Tail) // constructor form
```

Both pin the reply type and check the continuation. The trait form did it
through `Request::Reply` and needed the `messages!` macro to generate a
request type per variant; the constructor form does it through a field and
needs nothing. The guidance's own rule is that the two interaction forms
should teach each other. They competed instead, and every document and
example had to explain a difference that was not one.

What the trait form uniquely offered was bare request names at the call
site (`Tail(1)` rather than `AuditMessage::Tail(1, reply_to)`), request
types as values a test could search a batch for (`request_intents::<Tail>`),
and request types as generic parameters. Nothing outside the macro's own
tests used the last; the first is a glob import; the second is replaced by
inspecting a single command.

## Decision

A Command names its destination through one trait and delivers a value of
that destination's message type:

```rust,ignore
pub trait Addressable { type Message; }        // Port<M>, Connector<M>, ComponentRef<C>
Command::send(target, message)
Command::request_to(target, |reply_to| M::Variant(args, reply_to), map)
Command::request_to_timeout(target, constructor, timeout, map)
PortHandle::send(message) / request_to(constructor) / request_to_timeout(constructor, timeout)
```

A request variant carries its own `ReplyTo<R>`; the provider matches it and
calls `Command::reply(reply_to, value)`. The pairing of request and reply is
stated once, in the variant, and checked at every call site through the
constructor's signature. This is Elm's shape: the message type is the
protocol, constructors are its public surface, and the reply type comes from
the `expect`-style argument.

Removed: `messages!`; `Notification<M>` and `Request<M>`;
`RequestInvocation<M, R>`; `Command::notify`, `request`, `request_with`,
`request_timeout`, `request_timeout_with`; `PortHandle::notify`, `request`,
`request_timeout`; `Command::notification_intents` and `request_intents`;
`TraceCommandKind::Notify`. A Port-addressed send is traced as `Send` with
`target_port` set and `target_component` empty, which is the same record the
removed `Notify` produced under another name.

Added for unit tests: `Command::send_intents::<M>()` searches batches for
sends of `M`; `Command::into_request_to::<M>()` extracts a top-level request
as a `RequestIntent` holding the message built with an inert `ReplyTo` and a
`reply(outcome)` method that runs the continuation. The inert token belongs
to no Program, so a reply through it is rejected as foreign.

## Alternatives considered

- **Keep both forms.** Rejected for the reason above: one guarantee, two
  spellings, and the docs would carry the difference forever.
- **Keep `messages!` as sugar over the constructor form**, generating
  constructor functions instead of request types. Deferred: if
  `M::Variant { .. }` at call sites proves noisy after the examples are
  migrated, a small derive that emits constructor functions is the right
  tool, and it would not reintroduce a parallel API.
- **A `Fn` bound on request constructors** so batches could be inspected
  without consuming the command. Rejected: it would forbid moving owned
  arguments into the constructor, a cost on every call site to serve tests.

## Consequences

- No change to transition purity, per-Component serialization, causal
  ordering, controlled determinism, request correlation, or deadline rules.
  The Request arm of both runtimes already handled the constructor form; the
  Send arm now resolves Port bindings as the removed Notify arm did.
- The controlled trace no longer emits `TraceCommandKind::Notify`. Existing
  trace consumers that matched on it match `Send` with a `target_port`.
- A request variant's fields are visible where the data is, including the
  reply token; `#[derive(Debug)]` works because `ReplyTo` implements it.
- `Command::send` erases its target into an `Address`, one `Arc` clone per
  send for Port and Connector targets and one allocation for a
  `ComponentRef` target (ADR-0014's ledger entry, unchanged).
- Roughly a thousand lines leave the crate, four glossary entries retire,
  and a newcomer learns one sending story.

## Evidence

`tests/addressing.rs` (send and request_to through a Port, timeout, the
inspector, foreign-Port initial send, `PortHandle` send and request_to live);
`tests/connectors.rs`; every migrated suite and example passing with the
same assertions under the constructor form.

## Rollback

Revert the implementing commits. The constructor form was additive in
`2db81d1`, so reverting only the removal commit restores both forms.
