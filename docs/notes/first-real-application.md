# First Real Application Learnings

- Status: Exploratory notes
- Date: July 25, 2026
- Evidence: [`examples/shane.rs`](../../examples/shane.rs)
- Contract impact: Follow-through accepted by ADR-0005 through ADR-0008;
  remaining notes are exploratory

## Purpose

The first small application using Samara outside its designed acceptance
examples polls Coinbase's HTTP time endpoint, stores the latest time in a
Component Model, and reports the result. This document records where that
application feels direct and where the API makes the author express mechanism
instead of intent.

These are product and API learnings, not accepted requirements. They should be
resolved through API sketches and, where observable semantics change, an ADR
before implementation.

## What Already Feels Samaric

The application's central shape is good:

- `CliTimeModel` is the single source of mutable application truth.
- Messages name the inputs that can change that Model.
- Fetching network time is an explicit, first-party `HttpRequest` EffectDescriptor.
- The pooled HTTP client lives behind Samara's runtime-owned Driver; it cannot
  mutate the Model.
- The effect's terminal outcome returns through a pure Message mapper.
- Live assembly names the Component and its world binding explicitly.

The flow from a refresh event, through a finite HTTP interaction, back to a
state-changing Message is visible in one short `update` branch. That is the
kind of direct expression Samara is intended to enable.

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
`CliTimeServer` as both Component and separately constructed `GetTime` Driver.
The Component now owns only its application behavior and request intent.

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
let input = program.source::<StreamDescriptor<Input>>();
program.component(
    ComponentId::new("time-display"),
    TimeDisplay { network_time, input },
);
```

Effect Commands and Source Subscriptions require those stored capabilities. A
separate `uses_effect` manifest cannot drift because no raw-descriptor issuance
path remains. `ProgramBuilder::build()` closes the declarations, then both live
and controlled profile builders validate every terminal requirement
synchronously—even one first used only after a later Message.

The example now stores three Effect capabilities—HTTP, stdout, and stderr—and
declares them in three adjacent assembly calls. That is one field, one
declaration, and one borrowed issuance argument per logical world dependency.
The extra ceremony is visible, but it is also an accurate inventory of the
Component's authority. One composed Source capability similarly covers the
application-visible composed descriptor type and its lowered terminal
requirement; it does not require a second capability for the inner descriptor.

This supersedes ADR-0004's accepted dynamic-missing-binding allowance. A
deliberately hidden capability from another Program can still evade field
inspection, but it is non-conforming code and faults before terminal behavior;
it is not supported dynamic assembly.

## 3. Default Model Initialization

The common no-special-initialization spelling is currently:

```rust,ignore
Init::new(Model::default())
```

Consider implementing `Default for Init<Model, Message>` when `Model: Default`,
so a Component can write:

```rust,ignore
Init::default().with_command(initial_command)
```

This is a small ergonomic improvement with unsurprising Rust precedent. Its
type inference and interaction with subscriptions should be checked in real
examples before adoption.

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

The example implements repetition by scheduling `Command::after` in `init` and
then scheduling another timer whenever the tick Message is handled. The ongoing
desire to receive periodic ticks is a natural Subscription:

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
  one terminal outcome. `HttpRequest` is the correct first-party
  EffectDescriptor; decoding its response is application logic.
- **Subscription:** the Component's ongoing desire for periodic refresh events
  is naturally an interval Subscription. Each tick can issue one HTTP effect.
- **Port:** no Port is needed while this Component directly owns the use case.
  A Port becomes appropriate if a separate time-service Component exposes a
  provider-neutral `ReadTime` Request to this or several consumers. That
  provider would still realize the request through an HTTP effect or another
  terminal world boundary.

The current example therefore has the right fundamental model. Most of its
friction points are missing library affordances and assembly guarantees, not a
misuse of Component communication.

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

## Third Follow-Through: External Port Ingress

The example now exposes its current-time query as a provider-neutral
`TimeServerProtocol` Request. Surrounding Tokio code obtains a live handle from
the same assembled Port before spawning the runtime:

```rust,ignore
let time_server = runtime.port_handle(&port)?;
let runtime_task = runtime.spawn();
let current_time = time_server.request(GetCurrentTime).await?;
```

[ADR-0006](../adr/0006-live-port-ingress.md) defines this as a live host
boundary, not a capability available inside Component transitions. The host can
await `R::Reply` directly because it already has an ordinary async stack frame;
a Component-issued Request still returns through its Message continuation.
Both paths use the same Port binding, provider Message conversion,
`RequestInvocation`, opaque correlation, and `Command::reply` mechanism.

This makes the small application useful from both directions: its Component
continues to perform explicit HTTP and standard-output effects, while its host
can query application state without depending on the Component's private
Message enum or bypassing its serialized transition path.

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
