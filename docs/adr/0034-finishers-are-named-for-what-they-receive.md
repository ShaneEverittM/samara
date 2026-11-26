# ADR-0034: Finishers Are Named for What They Receive, and `divert` Marks a Result That Does Not Come Back

- Status: Accepted. Extended by
  [ADR-0035](0035-one-builder-pattern-for-effects-requests-and-http.md):
  requests gain `on_reply`/`on_failure` and `divert_to`, HTTP becomes steps
  on the effect builder, and `into_command()` is removed.
- Date: November 26, 2026
- Deciders: Samara maintainers
- Amends: [ADR-0023](0023-boundary-work-builders-and-finishers.md) (the
  finisher names) and [ADR-0025](0025-bare-effect-outcomes.md) (how an
  effect that cannot fail is spelled)
- Scope: `EffectBuilder`, `RequestBuilder`, `Diverted`, `OnSuccess`,
  `Destination`; `!` as the error type of an effect that cannot fail

## Context

ADR-0023 gave builders the finishers `producing`, `into_command`,
`finish_with`, and `discard_outcome`; `succeeding` and `replying` were
added later. Three problems had accumulated.

- **A reader could not tell which Commands come back.** In `update`
  nearly every Command returns as a Message. `finish_with` and `replying`
  do not, and nothing in their names said so.
- **The names had no common rule.** `succeeding(f)` ended an effect that
  cannot fail, while `replying(r).succeeding(f)` was the first of two arms
  and ended with `.failing(g)`, or with `.into_command()` when there was
  no failure to handle.
- **`replying` covered one destination.** Sending the result to another
  Component still needed the general closure.

## Decision

A finisher is named for what its function receives. One word, `divert`,
marks every result that does not return to the Component that issued the
work.

| Function receives | Comes back here | Goes elsewhere |
| --- | --- | --- |
| The whole outcome | `.on_outcome(f)` | `.divert_to(d).on_outcome(f)` |
| Success, then failure | `.on_success(f).on_failure(g)` | `.divert_to(d).on_success(f).on_failure(g)` |
| Success; the effect cannot fail | `.on_success(f)` | `.divert_to(d).on_success(f)` |
| The outcome, returning Commands | | `.divert_with(f)` |
| Nothing | | `.discard()` |

```rust,ignore
File::read(&cx.io.file).on_outcome(Message::Loaded)
Clock::system_time(&cx.io.clock).on_success(Message::Time)

File::read(&cx.io.file)
    .divert_to(reply)
    .on_success(move |contents| Ok(tail(&contents, count)))
    .on_failure(|_| Err(Error::CouldNotReadJournal))
```

- `divert_to(d)` takes a `Destination`: a `ReplyWith<R>` token, or a
  reference to anything `Addressable`. It changes only where the value
  goes. The finishers after it have the same names, and their functions
  return the value to deliver instead of a Message.
- On an effect that can fail, `on_success` returns an `OnSuccess` that is
  not a Command; `on_failure` completes it. A fallible effect cannot be
  finished with one arm.
- On an effect whose `Error` is `!`, `on_success` is the whole finisher
  and returns the Command. There is no failure arm to give and no extra
  call to end the chain.

  The two are different methods with one name. The finishing one is an
  inherent method on builders whose descriptor has `Error = !`; the
  two-arm one comes from the `FallibleEffect` trait, implemented for every
  builder. Rust prefers an inherent method when its impl applies and falls
  back to the trait method when it does not, so the error type selects
  between them. `FallibleEffect` is in the prelude; a file that imports
  names one by one needs it to call `on_success` on a fallible effect.
- `divert_with(f)` is the former `finish_with`, unchanged in meaning.
- `discard()` is the former `discard_outcome`. It is not behind
  `divert`: the word already says nothing comes back, and it is the most
  common finisher.
- `into_command()` (`Message::from`) is unchanged.
- Requests have `on_outcome`, `into_command`, and `divert_with`. A
  `RequestOutcome` has four cases, so `on_success`/`on_failure` and
  `divert_to` are not offered until the common shape is known.
- An effect that cannot fail declares `type Error = !;`. Rust 1.100
  stabilized the never type and made `Infallible` an alias for it, so this
  is a change of spelling only.

No semantics change. `divert_to` and its finishers are `divert_with`
underneath: one hop, no Message, no transition, the Model not visible.

## Alternatives considered

- **A separate starter**, `.divert().replying(reply)` or
  `.divert().by_replying_to(reply)`. Makes the marker its own call, but
  every destination then needs its own method name. Taking the destination
  as the argument needs one method and extends to new destinations.
- **`forward`** as the marker. In other actor frameworks it means passing
  the request on, which here is done by moving the reply token into a
  Message.
- **`pipe_to`**, after Akka. Recognisable, but it only says where the
  result goes. `divert` says this is the exception to results coming back.
- **Requiring `to_self()` for results that come back.** Uniform, but it
  marks the rule on every line rather than the exception.
- **Gerunds throughout** (`producing`, `yielding`, `succeeding` /
  `failing`). `succeeding` reads oddly as a step that is not the last,
  and the family does not extend to a request's four cases.
- **`Result`'s names** (`map`, `map_err`, `and_then`). `map` on a
  `Result` touches only success, and `and_then` means chaining another
  fallible step, which is the multi-hop sequencing ADR-0023 forbids.
- **Elm's `perform` / `attempt`.** `Effect::perform` is the builder's
  entry, and `attempt(f)` does not say what `f` receives.
- **`on_value` for the effect that cannot fail**, a separate name beside
  `on_success`. It was the first form built. A reader has to learn that
  "value" means "success, when failure does not exist"; `on_success` with
  nothing after it says the same thing without a new word.
- **`on_success(f).finish()` for the effect that cannot fail.** Not
  needed once the inherent method and the trait method share the name.

## Consequences

- Every finisher call is respelled. `producing` was the most used method
  in the code base.
- `on_` suggests an event handler. These functions are pure mappers that
  return a value; the documentation says so on each.
- `Replying` and `ReplyingSuccess`, added earlier the same day, are
  removed.

## Evidence

`tests/diverting.rs` covers each cell of the table for effects: a diverted
reply through both arms, through a lone `on_success` for an effect that
cannot fail, and through `on_outcome`; a
result diverted to an address; and the arms coming back as Messages
without `divert`. A compile-fail doctest covers a fallible effect given
only its success arm. The rest of the suite runs under the
new names.

## Rollback

Revert the implementing commit.
