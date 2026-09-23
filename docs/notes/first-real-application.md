# First Real Application Learnings

- Status: Exploratory notes
- Date: July 25, 2026
- Evidence: [`examples/time`](../../examples/time/src/main.rs)
- Contract impact: Follow-through accepted by ADR-0005 through ADR-0009;
  remaining notes are exploratory

## Purpose

The first small application using Samara outside its designed acceptance
examples now has two Components. `HttpTimeServer` polls Coinbase's HTTP time
endpoint and stores its latest observation. `Cli` consumes terminal lines,
either prints caller-supplied text or requests that observation through a
provider-neutral Port, and presents the reply. This document records where the
application feels direct and where the API makes the author express mechanism
instead of intent.

These are product and API learnings, not accepted requirements. They should be
resolved through API sketches and, where observable semantics change, an ADR
before implementation.

## What Already Feels Samaric

The application's central shape is good:

- `HttpTimeModel` and `CliModel` are the only mutable truth owned by their
  respective Components.
- Messages name every input that can change either Model.
- Fetching network time is an explicit, first-party `HttpRequest` EffectDescriptor.
- The pooled HTTP client lives behind Samara's runtime-owned Driver; it cannot
  mutate the Model.
- The effect's terminal outcome returns through a pure Message mapper.
- Terminal input is an ongoing, first-party `StdinLines` Source, not an
  ambient read hidden inside `Cli::update` or a host-owned forwarding thread.
- Standard output and error are explicit, first-party Effects.
- The CLI's time query is an explicit Request through `TimeServerProtocol`,
  with its outcome returning as a `CliMessage`.
- Program assembly names both Components, their Port relationship, and every
  world binding explicitly.

The two application flows remain easy to follow. A timer Message issues one
finite HTTP interaction whose outcome returns as a state-changing Message. A
terminal line becomes a CLI Message, which either issues one output Effect or
one time-service Request; the reply returns to the same Component as another
Message. That is the kind of direct expression Samara is intended to enable.

The accepted fluent response pipeline now lets the example state its response
intent directly as `on_response().require_success().json::<TimeResponse>()`.
Those steps are pure mapper behavior over the same raw HttpRequest outcome;
they do not move status or decoding policy into the terminal Driver or runtime.

The first draft exposed two places to improve:

- `println!` and `eprintln!` are side effects inside `update`. Reporting to the
  terminal belongs behind another explicit effect or a deliberately designed
  host-output boundary. The first follow-through below now replaces those
  calls with standard-output Effects.
- The first draft's `FailedToGetTime` discarded the typed error. The current
  Message carries application-level HTTP, status, decode, or cancellation data.

The first-party HTTP binding also removes the first draft's confusing reuse of
one type as both Component and separately constructed `GetTime` Driver. The
current `HttpTimeServer` name describes the concrete provider, while `Cli`
owns command interpretation and presentation. Each Component owns only its
application behavior and declared intent.

## 1. Host Lifecycle and `run_forever` (Implemented)

The first host draft spawned the runtime, slept for an arbitrary duration, and
then requested shutdown. A runtime fault could therefore sit unobserved until
the host eventually called `shutdown`; dropping the task first could make that
fault appear silent.

The natural long-running host API is now
`RuntimeTask::run_forever(&mut self)`. It:

- surfaces runtime termination and faults immediately;
- integrates cleanly with Ctrl-C or another host shutdown future;
- preserves structured ownership of every runtime task;
- leaves the selected Drain or Cancel policy explicit at the later
  `RuntimeTask::shutdown` call; and
- avoids requiring applications to invent a sleep loop merely to keep Samara
  alive.

The mutable borrow is deliberate: cancelling a pending observation in
`tokio::select!` leaves the `RuntimeTask` owning the live scope, after which the
host can consume it with explicit Drain or Cancel. The method accepts no signal
future and hides no host-future result; Ctrl-C errors, supervisors, deadlines,
and escalation remain ordinary host policy. ADR-0007 specifies the terminal
lifecycle boundary. It is not the public live trace observer deferred beyond
v0.

## 2. Validate Effect and Source Bindings During Profile Build (Implemented)

The application initially omitted its HTTP Effect Driver binding. Because the
effect first appeared after a timer Message rather than in `init`, the runtime
spawned successfully and faulted only when that branch of `update` issued the
effect.

ADR-0008 resolves the problem with Program-issued `EffectCapability<D>` and
`SourceCapability<S>` values. Declaring the dependency and obtaining the only
supported means of using it are one operation:

```rust,ignore
let network_time = program.effect::<HttpRequest>();
let input = program.source::<StdinLines>();
program.component(
    ComponentId::new("cli"),
    Cli { input, /* other declared capabilities */ },
);
```

Effect Commands and Source Subscriptions require those stored capabilities. A
separate `uses_effect` manifest cannot drift because no raw-descriptor issuance
path remains. `ProgramBuilder::build()` closes the declarations, then both live
and controlled profile builders validate every terminal requirement
synchronously—even one first used only after a later Message.

The example now declares three Effect capabilities—HTTP, stdout, and stderr—and
one `StdinLines` Source capability in adjacent assembly calls.
`HttpTimeServer` receives HTTP and stderr authority; `Cli` receives terminal
input, stdout, stderr, and the time-service Port. Cloning stderr makes shared
authority explicit without giving either Component undeclared access to other
Effects. The extra ceremony is visible, but it is also an accurate inventory
of each Component's authority.

This supersedes ADR-0004's accepted dynamic-missing-binding allowance. A
deliberately hidden capability from another Program can still evade field
inspection, but it is non-conforming code and faults before terminal behavior;
it is not supported dynamic assembly.

## 3. Default Model Initialization (Implemented)

The common no-special-initialization spelling is now:

```rust,ignore
Init::default()
```

`Default for Init<Model, Message>` is available when `Model: Default`, so a
Component can append initial work without spelling out the Model construction:

```rust,ignore
Init::default().with_command(initial_command)
```

Both Components in the example use this spelling. `Cli` appends its initial
usage message, while `HttpTimeServer` appends its first timer Command. The
result is a small ergonomic improvement with unsurprising Rust precedent.

## 4. More First-Party World Boundaries

### HTTP Effect and Driver — Implemented First Cut

[ADR-0005](../adr/0005-first-party-http-effect.md) resolves the narrow first
cut. `HttpRequest` owns method, URL text, headers, and body bytes;
`HttpResponse` owns raw status, version, headers, and fully buffered body bytes;
and `HttpError` distinguishes configuration from transport failure.

`bind_http()` retains one reqwest client and connection pool for its runtime
binding. It explicitly disables redirects, protocol retries, and system proxy
discovery, as well as automatic content decompression. HTTP statuses—including
3xx, 4xx, and 5xx—remain successful raw responses. The Coinbase status check
and JSON decoding stay in the example's pure response pipeline rather than
becoming hidden Driver policy. The only synthesized request header is the
no-preference transport default `Accept: */*` when the descriptor omits one.

This is deliberately not a mature HTTP stack. Streaming bodies, body limits,
timeouts, authentication, configurable pools, redirect/retry Layers, and typed
endpoint helpers remain future design work driven by additional applications.

### Persistent State in Manual Effect Drivers

Some Drivers need operational state shared across effect invocations: a pooled
HTTP client, authenticated session, device handle, or long-lived connection.
The registered `EffectDriver` object already persists for the live runtime's
lifetime and can hold such state. However, `execute(&self)` returns an owned
`'static` future, so custom Drivers commonly need cloneable handles or
`Arc`-backed interior state to move access into that future.

The first-party HTTP Driver now provides concrete evidence for this model: one
registered Driver instance retains a cloneable reqwest client, and sequential
effects reuse its pool. No additional runtime-owned state-store abstraction was
needed for that case.

We should determine whether this existing persistent Driver instance is a
sufficient escape hatch once documented and exemplified, or whether manual
Drivers need a more ergonomic runtime-owned store/context. Any design must make
concurrency semantics explicit: silently serializing all invocations through
one mutable store would be a consequential policy, not a free convenience.

A long-lived resource that continuously emits events may instead be a Source.
A resource exposing a reusable domain service to several Components may belong
behind a provider Component and Port. Neither alternative eliminates the
terminal world Driver; it gives ownership and policy a clearer home.

### Interval SourceDescriptor

The expanded example exercises a real Subscription for terminal input, but it
still expresses periodic polling with finite timer Commands. `HttpTimeServer`
implements repetition by scheduling `Command::after` in `init` and scheduling
another timer whenever the tick Message is handled. The ongoing desire to
receive periodic ticks could instead be a natural Subscription:

```rust,ignore
Subscription::source_with(
    &self.interval,
    SubscriptionId::new("refresh-time"),
    Interval::every(Duration::from_secs(1)),
    |_| Message::RefreshDue,
)
```

A first-party interval SourceDescriptor would give live execution Tokio time
and controlled execution logical time through the same Component program. It
would also stop naturally when the Subscription is withdrawn or the runtime
begins Drain.

The semantics need care. A fixed-rate interval, a fixed delay after completion,
and "never overlap requests" are different application intents. We must define
the first tick, missed-tick behavior, coalescing, and controlled-time behavior
rather than copying Tokio defaults accidentally. A Component may still need an
`in_flight` Model field if interval ticks must not start concurrent HTTP
effects. Scheduling the next timer only after the prior outcome remains the
more direct spelling for delay-after-completion polling.

## 5. Effect Declaration Ceremony

The first draft's block beginning at `GetTime` mostly existed to say that one
finite HTTP effect produces a response or error. The first-party descriptor
removes that protocol scaffolding while preserving the typed intent and
outcome. The remaining `GetTimeError` is useful application policy: it explains
transport failure, non-success status, JSON decoding, or cancellation.

Possible pressure-release points include:

- first-party descriptors and Drivers for common boundaries such as HTTP
  (implemented for the raw first cut);
- a derive or small macro for declaring a descriptor's Output and Error types;
- less boilerplate for explanatory error wrappers; and
- examples showing the shortest honest custom Driver, including persistent
  state.

The experiment also initially conflated ordinary Rust error-modeling ceremony
with Samara-specific ceremony. `thiserror` makes an intentional domain error
compact, and `EffectDescriptor::Error` only requires `Send + 'static`; a custom
wrapper is not required merely to satisfy Samara. The descriptor impl itself is
proportionate enough that a derive or macro should wait for more evidence.

Any shorthand must keep the descriptor, output, error, and live/controlled
boundary visible. "Ergonomic, but not magic" remains the constraint.

## Effect, Subscription, or Port?

For this application the split is:

- **Effect:** one HTTP request for the current time is finite and has exactly
  one terminal outcome. Printing one string is also finite and intentionally
  discards its best-effort output outcome. `HttpRequest`, `PrintStdout`, and
  `PrintStderr` therefore fit Effect semantics.
- **Subscription:** `Cli` has an ongoing, model-derived desire for terminal
  lines from `StdinLines`. EOF becomes `InputEnded`, failure becomes
  `InputFailed`, and either changes the Model and withdraws the Subscription.
  Periodic refresh remains a sequence of timer Commands for now; a first-party
  interval descriptor is still only an exploratory possibility.
- **Port:** `Cli` depends on a provider-neutral `TimeServerProtocol`, while
  `HttpTimeServer` happens to realize it using HTTP. `Command::request` carries
  `GetCurrentTime` to that provider, and the opaque continuation returns a
  `RequestOutcome<Option<DateTime<Utc>>>` as a CLI Message.

This division keeps world interaction, ongoing ingress, and Component
collaboration distinct without making the host interpret application commands.
The current example therefore has the right fundamental model. Most remaining
friction points are missing library affordances rather than a misuse of these
communication forms.

## Follow-Up Questions

1. Is persistent state on the Driver instance sufficient, and how should
   concurrent invocation be expressed?
2. Which interval semantics deserve the first-party name `Interval`?
3. Does another real custom Effect reveal enough repeated declaration ceremony
   to justify a derive or macro?

## First Follow-Through: Standard Output

The first follow-through from these notes is a deliberately narrow standard
output boundary: best-effort `PrintStdout` and `PrintStderr` descriptors plus an
explicit live `bind_stdio()` registration. They own UTF-8 text, offer a line
helper, deliberately hide host I/O errors, and remain ordinary controllable
Effects. This removes the example's need for ambient `println!`/`eprintln!`
without turning Samara into a logging framework. A future lower-level exact
byte-write API can expose `std::io::Error` when an application genuinely needs
to react to output failure.

The ergonomic follow-through is a root-qualified family of Samara formatting
macros. `samara::println!` and its stdout/stderr, line/no-line counterparts
take the corresponding output EffectCapability as their first argument and
produce discarded-outcome Commands: the runtime still owns and waits for the
print effect, but the Component does not need an artificial "printing
finished" Message. The capability argument is deliberate ceremony around
world authority. The macros remain visibly distinct from Rust's ambient,
unqualified `println!`.

## Second Follow-Through: Raw HTTP

The second follow-through is the pooled first-party HTTP effect specified by
[ADR-0005](../adr/0005-first-party-http-effect.md). The example now reads as a
raw `HttpRequest`, an explicit pure response pipeline, and `.bind_http()`. Its
custom descriptor, reqwest future, Driver, manual status/JSON helper, and
per-request client construction are gone. Loopback conformance evidence
verifies raw status handling, owned request data, controlled interception,
duplicate-binding rejection, and reuse of one HTTP/1.1 connection by
sequential effects.

The response pipeline lowers with `.into_command(&self.http)` (or the explicit
mapper variant), so its convenience syntax preserves the same declared HTTP
capability as a direct Effect Command.

## Third Follow-Through: First-Party Stdin and Component Port Request

An earlier revision used the live Port ingress from
[ADR-0006](../adr/0006-live-port-ingress.md) so surrounding Tokio code could
query the time Component. That remains a supported host boundary, but it is no
longer the flow exercised by this example.

The expanded application gives command interpretation to a `Cli` Component.
It declares `SourceCapability<StdinLines>`, reasserts that descriptor under a
stable Subscription identity, and maps each `SourceEvent` into its private
Message type. Live Unix assembly binds the exact capability with
`bind_stdin(&cli_input)`; controlled tests inject the same line, failure, and
EOF vocabulary through the ordinary typed Source boundary. The host no longer
owns an mpsc channel, forwarding thread, or input framing policy, and still
does not decide what `print` or `time` means.

[ADR-0009](../adr/0009-first-party-stdin-lines.md) makes the terminal mechanism
explicit. Each LF-delimited UTF-8 line becomes one String with LF and an
immediately preceding CR removed; an unterminated final line precedes normal
EOF. Invalid UTF-8 and operating-system reads become typed `StdinError`
failures. The Unix live binding waits for either stdin readiness or a private
cancellation channel on a runtime-owned reader thread. Source cutover and
runtime shutdown interrupt and join that thread, so Ctrl-C no longer waits for
Enter and no world-facing reader is detached.

Inside Samara, `print <text>` issues a standard-output Effect. `time` issues a
`GetCurrentTime` Request through the assembled `TimeServerProtocol` Port. The
runtime converts it into the provider's `RequestInvocation`, and
`HttpTimeServer` replies with `Command::reply`; the continuation returns as a
`CliMessage` for presentation. This exercises opaque request correlation and
Component-to-Component dependency injection without exposing either
Component's private Message enum.

`Cli` defines the application meaning of both terminal Source outcomes:
`InputEnded` and `InputFailed` record `input_closed`, and the next
`subscriptions` result withdraws input. EOF does not imply whole-program
shutdown: `HttpTimeServer` still owns recurring timer work, so the host remains
alive until Ctrl-C or runtime failure. That distinction keeps process lifecycle
as explicit host policy instead of assigning it accidentally to one Component's
Source. Focused tests exercise withdrawal, typed failure, line framing, EOF,
idle cancellation, exact binding validation, and print/request intent.

The CLI also makes one ordering boundary concrete. Two input lines reach the
Component in FIFO order and their transitions do not overlap, but a print
Effect from the first transition and a request/reply chain from the second are
independent asynchronous work. The later reply's presentation may therefore
reach the stdio Driver before the earlier print. That is consistent with the
current no-implicit-effect-completion-order contract. A CLI requiring ordered
transcript output would need to model that sequencing explicitly—for example,
by waiting for each mapped print outcome before issuing the next output—rather
than depending on incidental runtime scheduling. The simple example does not
claim that stronger policy.

## Fourth Follow-Through: Live Host Lifecycle

[ADR-0007](../adr/0007-live-host-lifecycle.md) adds
`RuntimeTask::run_forever(&mut self)` as a cancellation-safe terminal owner
observation. The example now selects it against the host's Ctrl-C future, so a
runtime fault surfaces as soon as structured cleanup completes and no arbitrary
sleep keeps the process alive.

When Ctrl-C wins, cancelling only the observation leaves `RuntimeTask` owning
the scope. The following call still names `Shutdown::Cancel` explicitly. The
library installs no signal handler and consumes no signal result; changing the
example to a service supervisor, test future, or deadline needs no Samara API
change.

## Fifth Follow-Through: Polling Ordering and Freshness

The September 22 review starts from `f215712`. Each `Tick` emits an HTTP
Effect and a one-second timer together. The timer is independent of the HTTP
outcome, so a slow call allows more calls to start. Every `TimeRetrieved`
unconditionally replaces `current_time`; completing a newer call before an
older one can then overwrite the newer observation. Serialized Component
transitions do not order independent effect completions.

### Application contract

Keep the current initial one-second delay and timer-driven cadence. The Model
is an enum with `Idle` and `Polling` states, each retaining the last observed
time. Busy ticks schedule the next tick but issue no HTTP work; missed polls are skipped, with no catch-up queue. A terminal
outcome makes the next scheduled tick eligible again. This differs from a
one-second delay *after completion*, which would move timer scheduling into the
outcome branches.

| State + Message | Next state | Commands |
| --- | --- | --- |
| Idle + Tick | Polling, retaining cached observation | One HTTP Effect and next one-second tick |
| Polling + Tick | Polling | Next one-second tick only |
| Polling + successful PollFinished | Idle, with new observation | None |
| Polling + failed/cancelled PollFinished | Idle, retaining cached observation | Print explanatory error |
| Idle + PollFinished | Idle, unchanged; no poll exists to complete | None |
| Either state + time-service Request | Unchanged | Reply with cached observation |

At most one HTTP poll is outstanding, and all state changes still occur through
Messages. A poll's terminal outcome is mapped exactly once by the existing
runtime contract. Consequently, two HTTP polls cannot race to overwrite the
cache. This does not promise monotonically increasing time from the remote
server; sequential observations may legitimately move backward.

An HTTP Effect that never terminates leaves the Model in `Polling` indefinitely,
while ticks and cached reads remain serviceable. Component Request timeouts do
not cancel or bound HTTP Effects. Scope Cancel still discards work without
fabricating an outcome; the Model then belongs to a terminated scope. No retry,
effect deadline, or runtime ordering policy is introduced by this fix.

Acceptance requires direct transition/Command tests plus controlled runs with
delayed completion, skipped ticks, recovery after each terminal outcome,
cached reads during work, and both orders of a tick/completion at the same
logical time. Rollback removes the guard and its tests together, reintroducing
overlap; changing to completion-driven polling instead must update the cadence
contract and evidence.

### Before/after evidence and API assessment

Before the fix, a controlled run issued polls A and B, accepted B's timestamp
200, then accepted A's timestamp 100 and overwrote the cache. All five new
regression tests failed against that implementation. The corrected program
prevents B from starting while A is pending. After A settles, another poll may
start on the next tick; a lower timestamp from that sequential poll is still
accepted.

The first guarded draft cleared the in-flight flag separately in success and
failure branches. The final Message shape uses one
`PollFinished(Result<DateTime<Utc>, GetTimeError>)`, retaining the existing
typed error mapping. Its common terminal transition returns `Polling` to
`Idle` for either result, making it harder to forget cleanup on one terminal
path. The existing `From<EffectOutcome<...>>` conversion supports this directly.

The retained evidence is in
[`polling_tests.rs`](../../examples/time/src/polling_tests.rs):

- `http_idle_ignores_unsolicited_poll_completion`: completion in Idle preserves
  state and emits no Commands, for both empty and populated caches.
- `http_poll_skips_busy_ticks`: initialization is Idle; the first tick enters
  Polling and repeated busy ticks emit no HTTP Effect.
- `http_poll_terminal_outcomes_update_state_and_allow_next_poll`: success,
  transport failure, non-success status, decoding failure, and cancellation all
  release the poll, preserve the intended cache behavior, and report errors.
- `controlled_slow_poll_skips_work_and_serves_cached_reads`: slow effects do not
  accumulate more polls; cache reads remain responsive, including before the
  first observation. Sequential remote time may move backward.
- `controlled_failed_polls_resume_on_next_tick_without_resetting_cadence`:
  recovery keeps the already-scheduled tick and never catches up missed polls.
- `controlled_tick_completion_orders_both_preserve_single_poll`: both orders
  of a tick and completion at the same logical instant are safe and repeatable.
  They may differ in whether that tick issues work, as allowed in live execution.

**Recommendation:** keep the runtime signatures and ordering semantics. Add
explicit polling guidance and a warning to `Command::batch` that batching HTTP
with a timer does not sequence them. Teach two distinct recipes: skip busy ticks
using Model state, or schedule the next tick from the terminal outcome for a
completion-driven delay. One example does not justify a generic concurrency API
with implicit operation identity or stale-result policy.

A future polling Layer/helper would need to name cadence, overlap, missed ticks,
terminal cleanup, and effect cancellation explicitly. An interval Source alone
would not fix overlap; an idempotency key alone would not establish freshness.
Keep those as design questions until another application demonstrates repeated
policy scaffolding. This example changes application policy only; the runtime's
serialization, causality, and controlled-time contracts remain unchanged.

### Enum Model experiment

The subsequent refinement replaces the boolean with `Idle { current_time }`
and `Polling { current_time }` and matches `(model, message)` in `update`.
Initialization names `Idle` explicitly. A completion in `Idle` is an explicit
no-op: it cannot change the cached observation or emit an error for nonexistent
work. This is an application transition rule, not a runtime stale-result check.

The two variants currently carry the same data, so they represent the same
state space as the boolean model. The benefit is named states, an exhaustive
transition table, and a place for future state-specific data. New states must
account for ticks and completion Messages; common cached reads stay shared.

The existing `update(&self, &mut Model, Message)` signature supports this
without cloning the Model or replacing it with a temporary default. The cached
timestamp is Copy; moving non-Copy payloads between variants would put more
pressure on this borrowed API and should be evaluated in a suitable example
before proposing an owned-Model transition signature.
