# ADR-0033: Context Fields Are `io`, `to`, and `config`

- Status: Accepted.
- Date: November 15, 2026
- Deciders: Samara maintainers
- Amends: [ADR-0031](0031-connections-configuration-and-context.md) (the
  field names of `Context`)
- Scope: `Context`

## Context

ADR-0031 named the fields of `Context` after their associated types:
`io`, `connections`, `configuration`. The type names are written once per
Component. The field names are written on every line that sends a Message
or reads a setting, and `cx.connections.` is fifteen characters of prefix
before the name that matters:

```rust,ignore
Command::request(&cx.connections.journal, ..)
```

The way to avoid it was to destructure in the parameter list, which cost
more than it saved:

```rust,ignore
fn update(
    Context {
        connections: Connections { journal },
        io,
        ..
    }: &Context<Self>,
    _: &mut Model,
    message: Message,
) -> Command<Message>
```

## Decision

The fields are `io`, `to`, and `config`. The associated types, the trait,
and the derive keep their names (`Io`, `Connections`, `Configuration`).

```rust,ignore
fn update(cx: &Context<Self>, _: &mut Model, message: Message) -> Command<Message> {
    match message {
        Message::Input(line) => Command::request(&cx.to.journal, ..),
        Message::Response(r) => samara::println!(&cx.io.output, "{r:?}"),
        Message::Tick => Command::after(cx.config.interval, ..),
    }
}
```

`to` reads as the sentence it is in: every connection is an address, and
every use is a send or a request to it. Beside `io` it keeps the prefix
meaningful to a reviewer: `io` is the world, `to` is another Component.

`Context::detached(connections, configuration)` and
`ComponentBuilder::configuration(..)` are unchanged.

## Alternatives considered

- **`Deref` from `Context` to its connections** (`cx.journal`). Shortest,
  but `journal` then looks like a field of `Context`, the line no longer
  says whether it touches the world or another Component, a connection
  named `io` or `config` would be shadowed, and a bare `Connector` as the
  whole `Connections` gains nothing.
- **A shorter concept word throughout** (`Links`, `cx.links`). Keeps field
  and type names matching but leaves the `Connector`/`connect` vocabulary
  and saves less.
- **Keep the names and bind locally**,
  `let Connections { journal } = &cx.connections;`. No API change; one
  line of boilerplate in every function.

## Consequences

- Two field names no longer match their associated type names. The
  mismatch is confined to the definition of `Context`.
- Names only. No change to behavior, identity, ordering, or controlled
  determinism.

## Evidence

Every Component in the tests, examples, and doc examples reads `cx.io`,
`cx.to`, and `cx.config`.

## Rollback

Revert the implementing commit.
