# ADR-0023: Boundary Work Is Built on What You Hold and Ends with a Finisher

- Status: Accepted for implementation trial. Spellings are expected to be
  revisited once the shape has been lived with.
- Date: November 2026
- Deciders: Samara maintainers
- Scope: How a Component describes effects, sends, requests, and replies;
  the finishers that turn a described interaction into a `Command`;
  `finish_with` and `Command<NoMessage>`; removal of the static `Command`
  constructors for boundary work

## Context

Two styles of describing boundary work had grown side by side. ADR-0022's
file capabilities are builders: `file.read()` returns an inert value that a
finisher turns into a Command. Everything else was a static function with
the finisher baked into its name: `Command::effect`, `effect_with`,
`effect_discarding_outcome`, `Command::send`, `request_to`,
`request_to_timeout`, `Command::reply`. Each new way for an outcome to end
would have needed a new function per entry point.

The pulse journal also showed a recurring cost. Answering a caller from an
effect's outcome took two private Message variants whose only job was to
carry the reply token across one hop. A continuation that could return
commands would remove them, but an unrestricted one lets a multi-step
workflow hide in a chain of closures, with its progress held in captures the
Model cannot see. Elm draws the same line: `Task.andThen` chains effects,
and nothing in a chain can do what a `Cmd` does.

## Decision

### The verb lives on the thing a Component holds

```rust,ignore
journal.send(JournalMessage::Transition { .. })      // Addressable: Port, Connector, ComponentRef, InstalledAssembly
journal.request(|reply| JournalMessage::Recent { count, reply })   // -> RequestBuilder
clock.perform(ReadSystemTime)                        // EffectCapability -> EffectBuilder
file.read()                                          // FileCapability   -> EffectBuilder (FileEffect is an alias)
reply.with(value)                                    // ReplyWith
```

`Command::` keeps only what has no target: `none`, `batch`, `after`.

### Every interaction ends with one of four finishers

| Finisher | The outcome becomes |
| --- | --- |
| `.producing(map)` | a Message, through a pure mapper |
| `.into_command()` | a Message, through `Message::from` |
| `.finish_with(f)` | finishing commands and no Message |
| `.discard_outcome()` | nothing (effects only) |

A request builder also takes `.timeout(duration)` before its finisher, which
replaces `request_to_timeout`.

### `finish_with` is exactly one hop

`f` receives the outcome and returns `Command<NoMessage>`. `NoMessage` has no
values and is not `Send`. The finishers that need a continuation require
`Message: Deliverable`, which every `Send + 'static` type satisfies and
`NoMessage` does not, so inside the closure only work that ends where it is
issued can be built: `reply.with`, `target.send`, an effect with
`discard_outcome`, `Command::none`, and batches of those. Starting anything
whose result would come back is a compile error with a diagnostic that says
so. The closure is pure and never sees the Model; state changes still
require a Message.

The runtimes already interpreted a Command continuation for effects (the
HTTP redirect Layer uses it). Requests gain the same path: a request's
erased continuation now yields either a Message or a Command, and both
runtimes interpret the latter under the requester with the outcome as its
trace cause.

## Alternatives considered

- **Add `finish_with` to each static entry point.** Three more function
  names today and three per future finisher.
- **Unrestricted Command continuations.** Rejected for the hidden-state
  reason above.
- **A distinct `Finish` type with its own constructors.** Airtight without
  the `Send` trick, but every finishing constructor would exist twice.
- **`then` or `and_then` as the name.** Both promise chaining, which is the
  one thing this forbids.

## Consequences

- No change to transition purity, per-Component serialization, causal
  ordering, or controlled determinism. A finishing command is traced as
  caused by the outcome, not by a transition, and adds no transition to the
  Component that issued it.
- A Component that answers a request from an effect result needs no private
  Message variants for the hop.
- `into_request_to` extracts only Message-producing requests; a finishing
  request is observed through controlled execution.
- `PortHandle`'s host-side `request_to` and the HTTP pipeline's
  `into_command_with` keep their names for now and are listed for the audit.
- Every boundary call site changes spelling. Mechanical.

## Evidence

`tests/finishers.rs`: an effect and a request both finished with
`finish_with`, with the transition count asserted so the absence of a
re-entry is observable; the finishing reply's trace cause is the effect
outcome; a request builder with `timeout` and `into_command`; inspectors
over builder-made commands; the same program live. The `finish_with` rustdoc
carries the compile-fail case for a continuation that tries to produce a
Message.

## Rollback

Revert the implementing commits. The builders were additive in the first
commit, so reverting only the removal restores both spellings.
