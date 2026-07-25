# First Real Application Learnings

- Status: Exploratory notes
- Date: July 24, 2026
- Evidence: [`examples/shane.rs`](../../examples/shane.rs)
- Contract impact: None yet; architecture changes require an ADR

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
- Fetching network time is an explicit, typed `EffectDescriptor`.
- The HTTP future lives behind an `EffectDriver`; it cannot mutate the Model.
- The effect's terminal outcome returns through a pure Message mapper.
- Live assembly names the Component and its world binding explicitly.

The flow from a refresh event, through a finite HTTP interaction, back to a
state-changing Message is visible in one short `update` branch. That is the
kind of direct expression Samara is intended to enable.

The first draft exposed two places to improve:

- `println!` and `eprintln!` are side effects inside `update`. Reporting to the
  terminal belongs behind another explicit effect or a deliberately designed
  host-output boundary. The first follow-through below now replaces those
  calls with standard-output Effects.
- `FailedToGetTime` discards the typed error. The Message should normally carry
  error data when the Component or operator can use it.

Using `CliTimeServer` as both the Component and the `GetTime` Driver is legal,
but those are two separately constructed instances playing different roles.
Names such as `TimePoller` and `CoinbaseTimeDriver` would teach the ownership
boundary more clearly once the example grows beyond a compact experiment.

## 1. Host Lifecycle and `run_forever`

The host currently spawns the runtime, sleeps for an arbitrary duration, and
then requests shutdown. A runtime fault can therefore sit unobserved until the
host eventually calls `shutdown`; dropping the task first can make that fault
appear silent.

We need a natural long-running host API, provisionally described as
`Runtime::run_forever`. It should:

- surface runtime termination and faults immediately;
- integrate cleanly with Ctrl-C or another host shutdown future;
- preserve structured ownership of every runtime task;
- make the selected Drain or Cancel policy explicit; and
- avoid requiring applications to invent a sleep loop merely to keep Samara
  alive.

The exact split among `spawn`, `wait`, `run_forever`, and signal integration is
unresolved. Fatal runtime completion is a host-lifecycle concern, not the
public live observer previously deferred beyond v0.

## 2. Validate Effect Bindings During `LiveRuntimeBuilder::build`

The application initially omitted its `GetTime` Driver binding. Because the
effect first appeared after a timer Message rather than in `init`, the runtime
spawned successfully and faulted only when that branch of `update` issued the
effect.

Desired direction: a Program with an unbound terminal effect or source
dependency should not spawn. Missing, duplicate, or ambiguous bindings should
fail synchronously during live runtime assembly.

The current Program cannot derive this complete set by inspecting arbitrary
`update` code, and Rust cannot enumerate all `EffectDriver<D>` implementations.
The design therefore needs an explicit declaration that cannot be accidentally
separated from issuing the Command. A typed effect capability, dependency
token, or related Port-like construction mechanism is one candidate:

```rust,ignore
let network_time = program.effect::<GetTime>();
program.component(
    ComponentId::new("time-display"),
    TimeDisplay { network_time },
);
```

If constructing `Command::effect` requires that registered capability, the
Program can retain the requirement and live `build()` can validate it. Merely
adding an optional `uses_effect::<GetTime>()` annotation would be weaker because
the author could forget both the annotation and the binding.

This revisits ADR-0004's accepted allowance for dynamically discovered missing
bindings and must be designed before changing the contract.

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

### HTTP Effect and Driver

A basic HTTP request currently requires the application to provide its own
descriptor, error wrapper, descriptor implementation, response types, Driver,
client construction, and live binding. A first-party HTTP effect would remove
incidental machinery while preserving the important intent as typed data.

The likely boundary is a finite `HttpRequest` EffectDescriptor interpreted by a
first-party `HttpDriver`. The Driver, rather than the protocol-neutral runtime
core, should own a reusable HTTP client and its connection pool. Response
decoding and application policy must remain explicit, pure Layers or
application logic rather than hidden Driver behavior.

Open design questions include request and response body types, header and
status representation, redirect and timeout policy, controlled outcomes, and
which pieces belong in compositional Layers.

### Persistent State in Manual Effect Drivers

Some Drivers need operational state shared across effect invocations: a pooled
HTTP client, authenticated session, device handle, or long-lived connection.
The registered `EffectDriver` object already persists for the live runtime's
lifetime and can hold such state. However, `execute(&self)` returns an owned
`'static` future, so custom Drivers commonly need cloneable handles or
`Arc`-backed interior state to move access into that future.

For HTTP, the first experiment should simply construct a
`CoinbaseTimeDriver { client: reqwest::Client }` once and bind that instance.
Reqwest's cloneable client already retains its connection pool. That will tell
us whether documentation is the missing affordance before Samara grows a new
state-store abstraction.

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
Subscription::source(
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

In the example, most of the block beginning at `GetTime` exists merely to say
that one finite effect produces `Time` or `GetTimeError`. The semantic value of
a named descriptor and typed outcome is real, but the current ratio of intent
to scaffolding is too low for such a small interaction.

Possible pressure-release points include:

- first-party descriptors and Drivers for common boundaries such as HTTP;
- a derive or small macro for declaring a descriptor's Output and Error types;
- less boilerplate for explanatory error wrappers; and
- examples showing the shortest honest custom Driver, including persistent
  state.

Some ceremony in this particular example is optional already:
`EffectDescriptor::Error` only requires `Send + 'static`, so `GetTime` could use
`reqwest::Error` directly and omit the hand-written string error wrapper. A
custom domain error becomes worthwhile when it deliberately hides transport
choice or adds application meaning, not merely to satisfy the trait.

Any shorthand must keep the descriptor, output, error, and live/controlled
boundary visible. "Ergonomic, but not magic" remains the constraint.

## Effect, Subscription, or Port?

For this application the split is:

- **Effect:** one HTTP request for the current time is finite and has exactly
  one terminal outcome. `GetTime` is correctly an EffectDescriptor.
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

1. What host API makes runtime completion, Ctrl-C, and shutdown policy read most
   directly?
2. What typed declaration makes every effect and source dependency knowable at
   live build without duplicating intent?
3. Is persistent state on the Driver instance sufficient, and how should
   concurrent invocation be expressed?
4. What is the smallest useful first-party HTTP descriptor without importing
   application policy into its Driver?
5. Which interval semantics deserve the first-party name `Interval`?
6. How much custom-effect ceremony can a derive or macro remove while keeping
   the boundary readable?
7. What explicit terminal-output effect should replace printing inside
   `update`?

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
produce discarded-outcome Commands: the runtime still owns and waits for the
print effect, but the Component does not need an artificial "printing
finished" Message. They remain visibly distinct from Rust's ambient,
unqualified `println!`.
