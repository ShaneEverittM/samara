# Samara API Sketch

This is a disposable, compiler-checked design workspace. It is not an
implementation and does not define Samara's public API yet.

The sketch should be evaluated against the rationale in
[`docs/api-guidance.md`](../../docs/api-guidance.md) and the vocabulary in
[`docs/glossary.md`](../../docs/glossary.md).

The provisional façade in `src/lib.rs` exists only so three consumer shapes can be
compiled and discussed:

- `src/bin/minimal.rs`: the canonical shallow onboarding path—one Counter, one
  first-party-shaped `mpsc -> Message` subscription, a normal `#[tokio::main]`
  entry point, and the same program assembled in a controlled test.
- `src/bin/api_pressure.rs`: a Counter requests capacity from a state-owning
  Quota provider through a Port, alongside `mpsc -> Message`, an effect
  descriptor, logical time, and equivalent live/controlled assembly.
- `src/bin/framed_socket.rs`: subscription reconciliation, framing in a Layer,
  a terminal source Driver, provider-neutral Ports, correlated request/reply,
  failure as a Message, and structured shutdown.

Check the sketch with:

```sh
cargo test --manifest-path design/api-sketch/Cargo.toml
```

The runtime methods are signatures, not implementations. `minimal` therefore
contains the intended runnable Tokio entry point but currently exits with the
façade's explicit placeholder error; its end-to-end controlled test is visibly
ignored for the same reason. Pure transition and subscription-shape tests run
today. The two API-pressure binaries remain intentionally inert.

## Current Assumptions

- A Component configuration contains immutable logical wiring. Its Model holds
  behaviorally relevant mutable state.
- `Component::update` receives no runtime context and may mutate an exclusively
  owned Model only as an observationally pure implementation technique.
- `Command<Message>` type-erases a typed `EffectDescriptor` plus a pure message
  mapper; tests can still inspect the concrete descriptor.
- Subscription identity is `(ComponentId, SubscriptionId)`. Equal source
  configuration remains active; changed configuration is replaced. Mapper
  identity is not part of reconciliation.
- `SourceEvent` distinguishes an item, an error-bearing failure, and normal end.
  `EffectOutcome` distinguishes the single terminal success, failure, or
  cancellation of one issued effect descriptor.
- `Framed<TcpBytes, Decoder>` is a Layer: it keeps transport I/O world-facing
  while running the same pure, stateful decoder in live and controlled profiles.
- A Driver is the terminal live-world implementation of an effect or source
  boundary. Controlled execution supplies outcomes and events without invoking
  live Drivers. “Adapter” is only the conceptual umbrella for Layers and Drivers.
- A named `Port<P>` is immutable Component wiring. `P::Message` defines a stable
  provider-neutral protocol vocabulary, and the provider's private Component
  Message implements `From<P::Message>`. Program assembly therefore selects the
  provider without repeating a conversion closure. Multiple named instances of
  the same protocol are distinct dependencies rather than one global type
  binding.
- `Request<P>::Reply` preserves the request/reply relationship statically.
  `Command::request` creates no future inside a Component: a one-shot request
  continuation turns the eventual `RequestOutcome<Reply>` into an ordinary
  requester Message.
- Providers receive `RequestInvocation<P, R>` with inert typed
  `ReplyTo<R::Reply>` authority and emit `Command::reply`. There are no oneshot
  senders or runtime handles in the transition boundary.
- `framed_socket` uses the optional `protocol!` macro for its health
  protocol as an ergonomics experiment. Each entry declares and generates one
  nameable operation type; the presence of `-> Reply` distinguishes requests
  from notifications. Provider-facing notification variants are flattened,
  while requests carry `RequestInvocation<P, R>`. Provider matching, private
  Component messages, Commands, and assembly bindings remain explicit. The
  public traits remain independently hand-implementable, as shown by the
  handwritten quota protocol in `api_pressure`.
- `ComponentRef<C> + C::Message` remains available for deliberately tightly
  coupled Components; Ports are the normal reusable dependency boundary.

## Questions to Poke

- Should transitions return one composable `Command`, a `Vec<Command>`, or an
  explicit `(Model, Vec<Command>)` value?
- Is framework-owned `Command<Message>` preferable to an application-owned
  command enum?
- Are arbitrary pure mapper closures acceptable, or should v0 begin with
  function pointers or stable mapper descriptors?
- Should subscription keys be strings, application-defined typed values, or
  both?
- Should each protocol own one `Message` enum as sketched, or should binding
  register each notification/request route separately?
- Does `protocol!` remove useful mechanical repetition without hiding intent?
  The presence of `-> Reply` distinguishes a request from a notification. The
  arrow remains required for a unit reply (`-> ()`). The prototype accepts only
  unit operations and operations with one tuple field. Its `type Protocol =>
  enum HealthProtocolMessage` declaration and comma-delimited body deliberately
  resemble the generated Rust surface. The prototype derives `Debug` for
  generated operation types and the protocol message enum. Attribute
  forwarding, multi-field operations, generics, and configurable derives remain
  open API questions rather than implied macro behavior.
- What establishes a request deadline: each `Command::request`, protocol policy,
  provider binding, or runtime default? The current API exposes timeout and
  cancellation outcomes without choosing that policy.
- Which request failures are meaningfully distinguishable without leaking
  mailbox or task topology? `RequestError` is intentionally provisional.
- Should notification delivery failure remain trace-only, or can a requester
  opt into a mapped delivery event without turning every notification into a
  request?
- Should Port bindings be allowed to form cycles freely, diagnosed as graph
  metadata, or restricted only when a stronger lifecycle rule requires it?
- Does the `Framed` Layer belong in Samara core or in a first-party composition
  library?
- Which state, trace, and shutdown inspection APIs are safe and useful in live
  execution?
