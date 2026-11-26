# ADR-0023: Boundary Work Is Named `Kind::verb` and Ends with a Finisher

- Status: Accepted. Amends the call spellings of ADR-0016 and ADR-0022.
  Extended November 26, 2026 with a fifth effect finisher, `replying`:
  `Effect::perform(..).replying(reply).succeeding(f).failing(g)` answers a
  request with the effect's result, each arm returning the reply value. It
  is `finish_with` for the commonest finishing work and adds no semantics:
  no Message, no transition, the same one hop. The builder holds the reply
  token, because two closures cannot both own something used once. A
  fallible effect must be given both arms; an infallible one ends with
  `.into_command()` after `succeeding`. It is intended as the template for
  further named finishing patterns. Requests have no `replying` yet: a
  `RequestOutcome` has more than two cases.
- Date: November 2026
- Deciders: Samara maintainers
- Scope: How a Component describes effects, sends, requests, and replies;
  the `Kind::verb` rule for where each entry point lives; the finishers that
  turn a described interaction into a `Command`; `finish_with` and
  `Command<NoMessage>`; removal of the finisher-in-the-name `Command`
  constructors

## Context

Two styles of describing boundary work had grown side by side. ADR-0022's
file capabilities were builders: `file.read()` returned an inert value that a
finisher turned into a Command. Everything else was a static function with
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

### Entry points are `Kind::verb`, rooted by what the work needs

World-facing work is rooted at the requirement that grants it. Work that
needs no capability is rooted at `Command`.

```rust,ignore
// Requirement-rooted: the first argument is the capability the Component declared.
Clock::system_time(clock)                            // -> EffectBuilder<ReadSystemTime>
File::read(file)                                     // -> EffectBuilder (FileEffect is an alias)
File::append_line(file, text)

// Command-rooted: needs no capability.
Command::send(&journal, JournalMessage::Transition { .. })
Command::request(&journal, |reply| JournalMessage::Recent { count, reply })   // -> RequestBuilder
Command::reply(reply, value)
Command::none()   Command::batch(..)   Command::after(duration, message)
```

`Command` by itself is therefore the root for work that requires no
capability, and every other root names the requirement that authorizes the
work. A reader can tell from the first token of a call whether it reaches
the world. Targets are `Addressable` (Port, Connector, ComponentRef,
InstalledAssembly, and references to them) and are passed by reference.

`send` and `reply` have nothing to observe and are Commands already.
`perform`, the `File` operations, and `request` return builders.

`Clock` is the Component-facing name for `Effect<ReadSystemTime>`.
`Clock::system_time(clock)` describes one read through that exact capability.
Drivers and controlled tests still handle the `ReadSystemTime` descriptor.
The generic spelling `Effect::perform(clock, ReadSystemTime)` remains equivalent,
including its binding checks, and callers can switch back to it independently.

### Finishers

| Finisher | The outcome becomes |
| --- | --- |
| `.producing(map)` | a Message, through a pure mapper |
| `.succeeding(map)` | a Message mapped directly from the output (effects with `Error = Infallible` only) |
| `.into_command()` | a Message, through `Message::from` |
| `.finish_with(f)` | finishing commands and no Message |
| `.discard_outcome()` | nothing (effects only) |

A request builder also takes `.timeout(duration)` before its finisher, which
replaces `request_to_timeout`. An effect's outcome is `EffectOutcome`
(ADR-0025); a request's is `RequestOutcome`.

### `finish_with` is exactly one hop

`f` receives the outcome and returns `Command<NoMessage>`. `NoMessage` has no
values and is not `Send`. The finishers that need a continuation require
`Message: Deliverable`, which every `Send + 'static` type satisfies and
`NoMessage` does not, so inside the closure only work that ends where it is
issued can be built: `Command::reply`, `Command::send`, an effect with
`discard_outcome`, `Command::none`, and batches of those. `Command::request`
is defined on `Command<NoMessage>` so that its target's message type is
inferred from the target alone; its finishers still require `Deliverable`,
so a request cannot be completed inside the closure. Starting anything
whose result would come back is a compile error with a diagnostic that says
so. The closure is pure and never sees the Model; state changes still
require a Message.

The runtimes already interpreted a Command continuation for effects (the
HTTP redirect Layer uses it). Requests gain the same path: a request's
erased continuation now yields either a Message or a Command, and both
runtimes interpret the latter under the requester with the outcome as its
trace cause.

`Component::shutdown` (ADR-0024) returns the same `Command<NoMessage>`, for
the same reason.

## Alternatives considered

- **Add `finish_with` to each static entry point.** Three more function
  names at the time and three per future finisher.
- **Verbs as methods on the held value** (`journal.send(..)`,
  `clock.perform(..)`, `file.read()`, `reply.with(..)`). Implemented first
  and lived with briefly. Discoverable through completion, but a transition
  arm no longer began with a word saying what kind of thing it returned, and
  capability-bearing calls looked the same as plain sends. The static form
  kept the builders and restored that reading.
- **`Command::perform` for effects.** Puts world work under the root that
  means "needs no capability", which is the distinction the rule exists to
  show.
- **`Connector::send` / `ComponentRef::send`.** Would make the root depend on
  which kind of address a Component happens to hold; `Addressable` exists so
  it does not have to.
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
- `Command::into_request` extracts only Message-producing requests; a finishing
  request is observed through controlled execution.
- `PortHandle`'s host-side `request_to` and `request_to_timeout`, the HTTP
  pipeline's `into_command_with`, and `Subscription::source_with` keep their
  older names. They are recorded in `docs/notes/framework-follow-ups.md`.
- The capability methods behind the `File` operations are private; the
  `File::` functions are the public surface (amends ADR-0022's spelling).
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
commit, so reverting only the removal restores both spellings. The
`Kind::verb` respelling is a separate, purely mechanical commit.
