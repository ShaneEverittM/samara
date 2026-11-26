# ADR-0035: One Builder Pattern for Effects, Requests, and HTTP

- Status: Accepted. Extended the same day: the step helper is public as
  `EffectBuilder::map_outcome`, so a library can give its own effect steps
  through an extension trait. The Layer hook that lets a step issue the
  effect again stays private.
- Date: November 26, 2026
- Deciders: Samara maintainers
- Amends: [ADR-0005](0005-first-party-http-effect.md) (the HTTP response
  pipeline), [ADR-0012](0012-http-redirect-layer.md) (how the redirect
  Layer is attached), [ADR-0016](0016-one-sending-api.md) (request
  finishers), [ADR-0023](0023-boundary-work-builders-and-finishers.md), and
  [ADR-0034](0034-finishers-are-named-for-what-they-receive.md)
- Scope: `EffectBuilder`, `RequestBuilder`, `Diverted`, `Arm`, `Work`,
  `Http`; removal of `HttpResponsePipeline` and `into_command`

## Context

ADR-0034 gave effects a regular set of finishers. Three things were left
outside it.

- **Requests** had `on_outcome` and `divert_with` only. A Component that
  asks another and relays the answer matched four `RequestOutcome` cases
  by hand.
- **HTTP** had its own builder, `HttpResponsePipeline`, entered from the
  descriptor and given the capability last, with its own two finishers:

  ```rust,ignore
  HttpRequest::get(url).on_response().require_success().json::<T>().into_command(&cx.io.http)
  ```

  None of the ADR-0034 finishers applied to it, and following redirects
  needed a dedicated Command.
- **`into_command()`** was `on_outcome(Message::from)` under another
  name, and the one finisher outside the naming rule.

## Decision

Boundary work is written one way:

```text
<entry point>.<steps specific to that work>.<finisher>
```

```rust,ignore
Http::get(&cx.io.http, url)
    .follow_redirects(5)
    .require_success()
    .json::<TimeResponse>()
    .on_success(Message::Time)
    .on_failure(Message::TimeFailed)

Command::request(&cx.to.gauge, GaugeMessage::Read)
    .timeout(Duration::from_secs(1))
    .divert_to(reply)
    .on_reply(|reading| Ok(reading + 1))
    .on_failure(|_| Err(Error::GaugeSilent))
```

- **Entry point.** `Effect::perform(&capability, descriptor)` for any
  effect, or a capability's own verb: `File::read(&file)`,
  `Clock::system_time(&clock)`, and now `Http::get(&http, url)` and
  `Http::send(&http, request)`, where `Http` is `Effect<HttpRequest>`.
  `Command::request(&target, constructor)` for a request.
- **Steps.** Methods that refine the work before it is finished. A
  request has `timeout`. HTTP has `follow_redirects`, `require_success`,
  and `json`, in that order. A library adds steps for its own effects
  with `EffectBuilder::map_outcome`, wrapped in an extension trait
  implemented for `EffectBuilder<ItsDescriptor>` (an inherent method is
  not possible from another crate), and an entry point as an associated
  function on a type of its own: `Key::read(&cx.io.store, "limit")`.
- **Finisher.** The ADR-0034 set, on both builders:

  | Function receives | Effect | Request |
  | --- | --- | --- |
  | The whole outcome | `on_outcome(f)` | `on_outcome(f)` |
  | One case, then the other | `on_success(f).on_failure(g)` | `on_reply(f).on_failure(g)` |
  | The only case | `on_success(f)` when `Error = !` | |
  | The outcome, returning Commands | `divert_with(f)` | `divert_with(f)` |
  | Nothing | `discard()` | |

  `divert_to(destination)` precedes any of the first three on either
  builder and sends the result to a reply token or an address.

Supporting decisions:

- `EffectBuilder<D, Output, Error>` carries the result's types as they
  stand after any steps. They default to the descriptor's, so
  `EffectBuilder<D>` means what it did. A step is a pure function from one
  outcome to the next. `require_success` and `json` are written this way
  and change `Error` to `HttpResponseError` and `Output` to the decoded
  type.
- A request's failure arm receives `RequestOutcome<!>`: the existing enum
  with the reply case made impossible. A `match` on it needs no `Replied`
  arm and no new type is introduced.
- Following redirects is a Layer the builder carries: a pure step that may
  ask for the effect to be issued again with another descriptor. It
  composes with every finisher. Each hop is still a separate `HttpRequest`
  effect through the same capability, visible to controlled tests, as
  ADR-0012 requires.
- `into_command()` is removed from effects and requests. Write
  `on_outcome(Message::from)` or `on_outcome(From::from)`.
- `HttpResponsePipeline`, `HttpRequest::on_response`, and the pipeline's
  `into_command(&capability)` and `into_command_with` are removed.

No semantics change. A builder with steps lowers to one effect Command
whose continuation runs the steps and then the finisher, each at most
once. `divert_to` remains `divert_with` underneath.

## Alternatives considered

- **A new enum for a request's failure arm.** It would repeat three of
  `RequestOutcome`'s four variants.
- **Folding `TimedOut` and `Cancelled` into `RequestError`**, so a request
  outcome is a reply or a typed error, as an effect's is. Cleaner, and it
  would let requests use `on_success`. It changes a public enum and the
  trace vocabulary and is left for its own decision.
- **Keeping `Effect::perform(&http, HttpRequest::get(url))` as the only
  HTTP entry.** It still works. `Http::get` matches `File::read` and
  `Clock::system_time` and removes a nested call from the common case.
- **Renaming `into_command()` to an `on_` name.** There is nothing to
  name: it takes no function.
- **Leaving the redirect Layer as a dedicated Command.** It could then
  end only in a Message, so a redirected request could not be diverted.

## Consequences

- An inert builder can be held and finished later, as `examples/url`
  does while it debounces: the type is `EffectBuilder<HttpRequest>` where
  it was `HttpResponsePipeline<HttpResponse, HttpError>`. It now holds the
  capability as well as the request.
- Request construction and response handling are no longer separated by a
  type. Set the method, headers, and body on the `HttpRequest` and pass it
  to `Http::send`.
- `Work`, `Destination`, and `Sink` are public traits that applications
  do not implement. They exist so `Diverted` and `Arm` can serve both
  builders.

## Evidence

`tests/diverting.rs` covers a request finished with `on_reply` and
`on_failure`, including a timeout arriving as `RequestOutcome::TimedOut`,
and a request diverted to a caller's reply token with no transition in the
relay. `tests/http_redirects.rs` covers redirects, a status check, and a
diverted reply in one chain. `tests/custom_steps.rs` builds a small
library on the public API alone, with an entry point and three steps, and
finishes it both ways. `tests/http_response_pipeline.rs`,
`tests/http_effects.rs`, and the `time` and `url` examples run on the
builder. Compile-fail doctests cover `json` before `require_success` and a
missing failure arm.

## Rollback

Revert the implementing commit.
