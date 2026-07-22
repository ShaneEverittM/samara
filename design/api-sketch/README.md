# Samara API Sketch

This is a disposable, compiler-checked design workspace. It is not an
implementation and does not define Samara's public API yet.

The sketch should be evaluated against the rationale in
[`docs/api-guidance.md`](../../docs/api-guidance.md) and the vocabulary in
[`docs/glossary.md`](../../docs/glossary.md).

The provisional façade in `src/lib.rs` exists only so three consumer shapes can be
compiled and discussed:

- `src/bin/minimal.rs`: the canonical shallow onboarding path—one Counter, one
  first-party-shaped `mpsc -> Msg` subscription, a normal `#[tokio::main]`
  entry point, and the same program assembled in a controlled test.
- `src/bin/api_pressure.rs`: a Counter requests capacity from a state-owning
  Quota provider through a Port, alongside `mpsc -> Msg`, an effect, logical
  time, and equivalent live/controlled assembly.
- `src/bin/framed_socket.rs`: subscription reconciliation, framing in an adapter,
  provider-neutral Ports, correlated request/reply, failure as a message, and
  structured shutdown.

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

- `Component::update` receives no runtime context and may mutate an exclusively
  owned model only as an observationally pure implementation technique.
- `Cmd<Msg>` type-erases a typed effect plus a pure result mapper; tests can still
  inspect the concrete effect intent.
- Subscription identity is `(ComponentId, SubscriptionId)`. Equal source
  configuration remains active; changed configuration is replaced. Mapper
  identity is not part of reconciliation.
- `SourceEvent` distinguishes an item, failure, and normal end. `EffectEvent`
  distinguishes success, failure, and cancellation.
- `Framed<TcpBytes, Decoder>` keeps transport I/O world-facing while running the
  same pure, stateful decoder in live and controlled profiles.
- A named `Port<P>` is immutable Component wiring. `P::Inbound` defines a stable
  provider-neutral vocabulary, and program assembly maps it into one provider's
  private `Msg` type. Multiple named instances of the same protocol are distinct
  dependencies rather than one global type binding.
- `Request<P>::Reply` preserves the request/reply relationship statically.
  `Cmd::request` creates no future inside a Component: a pure mapper turns the
  eventual `RequestOutcome<Reply>` into an ordinary requester message.
- Providers receive `Incoming<P, R>` with inert typed `ReplyTo<R::Reply>` data
  and emit `Cmd::reply`. There are no oneshot senders or runtime handles in the
  transition boundary.
- `framed_socket` uses the optional `protocol!` macro for its health
  protocol as an ergonomics experiment. Each entry declares and generates one
  nameable operation type; the presence of `-> Reply` distinguishes requests
  from notifications. Provider-facing notification variants are flattened,
  while requests carry `Incoming<P, R>`. Provider matching, private Component
  messages, commands, and assembly bindings remain explicit. The public traits
  remain independently hand-implementable, as shown by the handwritten quota
  protocol in `api_pressure`.
- `ComponentRef<C> + C::Msg` remains available for deliberately tightly coupled
  Components; Ports are the normal reusable dependency boundary.

## Questions to Poke

- Should `Component` be an instance containing logical configuration, or a
  static type operating only on `Model`?
- Should transitions return one composable `Cmd`, a `Vec<Cmd>`, or an explicit
  `(Model, Vec<Cmd>)` value?
- Is framework-owned `Cmd<Msg>` preferable to an application-owned command enum?
- Are arbitrary pure mapper closures acceptable, or should v0 begin with
  function pointers or stable mapper descriptors?
- Should subscription keys be strings, application-defined typed values, or
  both?
- Should each protocol own one `Inbound` enum as sketched, or should binding
  register each notification/request route separately?
- Does `protocol!` remove useful mechanical repetition without hiding intent?
  The presence of `-> Reply` distinguishes a request from a notification. The
  arrow remains required for a unit reply (`-> ()`). The prototype accepts only
  unit operations and operations with one tuple field. Its `type Protocol =>
  enum Inbound` declaration and comma-delimited body deliberately resemble the
  generated Rust surface. The prototype derives `Debug` for generated
  operation types and the inbound enum. Attribute forwarding, multi-field
  operations, generics, and configurable derives remain open API questions
  rather than implied macro behavior.
- What establishes a request deadline: each `Cmd::request`, protocol policy,
  provider binding, or runtime default? The current API exposes timeout and
  cancellation outcomes without choosing that policy.
- Which request failures are meaningfully distinguishable without leaking
  mailbox or task topology? `RequestError` is intentionally provisional.
- Should notification delivery failure remain trace-only, or can a requester
  opt into a mapped delivery event without turning every notification into a
  request?
- Should Port bindings be allowed to form cycles freely, diagnosed as graph
  metadata, or restricted only when a stronger lifecycle rule requires it?
- Does the `Framed` composition belong in Samara core or in a first-party adapter
  library?
- Which state, trace, and shutdown inspection APIs are safe and useful in live
  execution?
