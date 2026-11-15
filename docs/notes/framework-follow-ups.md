# Framework Follow-Ups

Exploratory suggestions, September 22, 2026. Observable semantic changes need
an ADR before implementation. Shutdown escalation is complete under
[ADR-0010](../adr/0010-shutdown-escalation.md).

Teaching principles for the future guide live in [User Guide Notes](user-guide-notes.md).
Related research: [Lessons from TypeScript Effect](effect-ts-research.md).

- **Request lifecycles:** Opt-in timeouts and harmless late replies are complete
  under [ADR-0011](../adr/0011-request-timeouts.md). Explicit per-request
  cancellation and provider abandonment remain deferred; provider work continues
  after timeout.
- **Ordering and freshness:** `examples/time` now uses an Idle/Polling Model
  enum, skips busy ticks, and tests delayed outcomes and both tick/completion
  orders. The
  [before/after API review](first-real-application.md#fifth-follow-through-polling-ordering-and-freshness)
  recommends explicit polling guidance and common terminal cleanup. Broader
  schedule variation and reusable concurrency APIs remain exploratory.
- **Operation identity:** Consider keyed operations where application identity
  is useful. Keep idempotency, invocation correlation, freshness, and sequencing
  distinct; require keys only when they enable a defined guarantee.
- **Bounded work:** Design admission and outstanding-work limits, including
  end-to-end pressure through Sources. Measure sustained-load memory and latency;
  leave rejection, coalescing, and shedding choices explicit.
- **Downstream Layers:** Implement a reusable reconnect/backoff composition
  outside the crate, unchanged across live and controlled execution, to discover
  the necessary public extension points.
- **Application evidence:** Exercise reconnects, competing operations,
  cancellation, and shutdown in one realistic application. Use it to assess
  Component boundaries and reduce repeated ceremony.
- **Optional integrations:** Feature-gate or split HTTP/JSON/TLS and platform
  integrations so consumers can use the runtime without unrelated dependencies.
- **Owned-Model transition signature:** The time example's enum Model works
  with `update(&Config, &mut Model, Message)` without cloning the Model or
  replacing it with a temporary default, because the cached timestamp is Copy.
  Moving non-Copy payloads between Model variants would put more pressure on
  this borrowed API. Evaluate that in a suitable example before proposing an
  owned-Model transition signature; changing the kernel signature in
  [the API contract](../api-contract.md) needs an ADR. Making the Model the
  implementing type (`Model = Self`) was considered on November 8, 2026 and
  declined for the borrowed signature (ADR-0013); it is the natural spelling
  of the owned one, so the two decisions go together.
- **Descriptor declaration shorthand:** A derive or small macro for
  declaring a descriptor's Output and Error types, and less boilerplate for
  explanatory error wrappers, should wait for evidence from another real custom
  Effect. The time example conflated ordinary Rust error-modeling ceremony with
  Samara-specific ceremony: `thiserror` makes an intentional domain error
  compact, and `EffectDescriptor::Error` only requires `Send + 'static`, so a
  custom wrapper is not required merely to satisfy Samara. The crate's `fs`
  descriptors and `FsError` are the next data point. Any shorthand must keep
  the descriptor, output, error, and live/controlled boundary visible.

- **Deferred API areas, recorded so they are not forgotten:** a blessed escape
  hatch and how it advertises weakened guarantees; named capability bundles and
  capability identity inspection; a universal `Layer` trait or other general
  profile-binding abstraction beyond type-wide Drivers and exact Source
  capabilities; notification delivery-failure semantics; domain-payload trace
  capture, typed trace projections, a public live observer, and durable trace
  storage; exact shutdown diagnostic counts. Each was listed in the vision or
  glossary before November 8, 2026 and now lives only here.

## URL Checker API Exercise — September 23, 2026

Open items from writing the application by hand; these are exploratory proposals,
not accepted API decisions or a review of the unfinished application.

- [x] **Example-led rustdoc.** Done September 24, 2026: public API docs in
  `src/lib.rs` lead with caller behavior and carry complete, compile-checked
  source, request/reply, HTTP, Driver, shutdown, and controlled-test examples.
- [ ] **Explicit Subscription restart revision:** Explore a Model-owned revision
  that the Component can increment to request a fresh Source realization with
  unchanged configuration. Consider keeping the stable Subscription identity
  separate from this revision; `with_revision(...)` is only a candidate spelling.
  Specify replacement, cleanup, and stale-event behavior using the existing
  private-generation cutover contract. Keep retry/backoff policy in Components
  or Layers and preserve deterministic controlled execution. Document binding
  limitations: TCP can reconnect, while a consumed one-shot `mpsc` receiver
  cannot be recreated by incrementing a revision. Test retention, restart,
  stale-event rejection, and unsupported reactivation before implementation.
- [ ] **Component completion and library supervision:** Explore per-Component
  `Command::stop(...)` and a library `Supervisor` Component, motivated by the
  checker's stdin EOF. The September 24 discussion favors keeping cooperative
  shutdown and supervision policy in ordinary Components, with only the required
  lifecycle mechanisms in the runtime. This is a design direction, not an
  accepted API; work through the questions below before implementation.
- [ ] **Composable Programs / assemblies:** Explore `urlchecker::program()` as
  an application library's public assembly boundary, shared by its executable
  and controlled tests. Let callers use deliberately exposed handles,
  capabilities, and protocols without knowing the internal wiring. Extend that
  idea to composing assemblies into a larger Program while keeping internal
  Components, timers, and capabilities private where appropriate.
  Separate assembly from execution: an assembly could contribute Components
  and binding requirements to one enclosing runtime, preserving runtime
  ownership and controlled execution without requiring a nested runtime.
  Define identity scoping, capability ownership, binding configuration, and
  protocol connections before choosing an API. An assembly boundary might also
  define a supervision group, but composition must not implicitly select shared
  restart or shutdown policy. Keep those choices explicit and coordinate this
  proposal with the supervision design below. This September 24 direction is
  exploratory; require an ADR and live/controlled composition examples before
  implementing semantic changes.
- [ ] **Isolated Component test runtime:** Explore a `ComponentRuntime` type for
  testing one Component's model, message handling, and emitted commands without
  assembling a full application. Support sequences with controlled timers,
  source events, and effect outcomes, not just individual update calls. Reuse
  the runtime's transition and command machinery so isolated tests preserve
  execution semantics. Define how capabilities are supplied and how external
  effects, sources, and ports are explicitly controlled or stubbed.
  Runtime and ControlledRuntime both accept a Program without sharing a
  runtime interface; preserve that flexibility and let ComponentRuntime expose
  APIs suited to isolated tests. Prioritize this over per-Component transition
  counts in integration tests, which should generally assert behavior across
  public boundaries. Component-scoped traces remain useful for diagnosis and
  targeted delivery guarantees. Keep transition counts defined as updates
  executed; tests can separately assert resulting model state and commands.
  This September 24 proposal is exploratory; settle construction, dependency
  boundaries, and conformance evidence before implementation.

## Pulse Monitor API Exercise — October 8, 2026

Open items from designing `examples/pulse`, a multi-target uptime monitor with
a journal Component; exploratory proposals, not accepted API decisions.

- [x] **Time access beyond delayed delivery:** The provider-effect shape
  landed November 8, 2026 as `ReadSystemTime` with `SystemClockDriver`
  (ADR-0013); controlled tests complete it with any instant. The subscription
  shape and the logical-clock-backed controlled driver below remain open.
  Original statement: Logical time currently reaches
  Components only through `Command::after`. That covers scheduling but not
  observation: a Component cannot learn the current time to stamp a journal
  entry, compute an age, or compare against a deadline without reading the wall
  clock inside `update`, which the purity rule forbids. The workaround is an
  application-defined `GetTime` effect with its own Driver, which is ceremony for
  something every real application needs. Explore two first-party shapes and
  decide which, or both, the runtime should own:
  - A time provider effect: a built-in descriptor whose outcome carries the
    current time, served by the runtime clock live and by the logical clock in
    controlled execution. This keeps time a message like any other outcome.
  - A time subscription: a `SourceDescriptor` that ticks at a declared interval
    and carries the time of each tick, reconciled like other subscriptions.
    This replaces hand-rolled `after` loops for periodic work and gives
    controlled tests one place to advance time.
  Either shape must keep `update` free of clocks, must be served from the same
  logical clock that drives timers so observed time and scheduled time agree,
  and must preserve deterministic controlled traces. Decide the representation
  of time (monotonic runtime instant, wall-clock timestamp, or both) and how a
  controlled test sets the epoch. Additive API, but it changes what controlled
  execution must supply, so write an ADR and matching live and controlled tests
  before implementation.

- [x] **Library assembly via `install`.** Landed November 7, 2026 and
  redesigned November 8 as the `Assembly` trait with a type-level `Requires`
  list, `with_assembly(..).and_drivers(..)` on the live builder and
  `with_assembly(..)` alone on the controlled one, and `VendorsDrivers`
  (ADR-0013); design, rejected alternatives, and remaining open items are in
  the [Assembly design note](assembly-design.md). Amended November 9 by
  ADR-0018 and ADR-0026: an Assembly declares no Ports and exports one
  value, usually a `Connector`. ADR-0021 resolves Component identity scoping with builder-issued IDs
  and optional diagnostic names. Still open: named `Requires` positions and a
  Port-level controlled ingress so an Assembly can be tested from outside its
  module.

- [x] **Connectors as first-class addresses.** Landed November 8, 2026
  (ADR-0014): `Command::send(&target, ..)` over any address, `Command::request(&target, ctor)` and
  `request_to_timeout` minting the `ReplyWith` against a Connector from a published
  constructor function, and `Connector::accepting`, Elm 0.16's `forwardTo`. A
  library written the Elm way, an opaque Message with constructor functions,
  now needs no shared enum and no Port; `tests/connectors.rs` has one.
  `Command::into_request` inspects such a request with an inert token
  (ADR-0016).

- [ ] **Declared Clock capability.** Titled as
  [ADR-0017](../adr/0017-declared-clock-capability.md), not yet written:
  `program.clock()` so timers and deadlines are declared like effects, a
  live runtime binds a wall, scaled, or paused clock or has no time, and
  controlled execution keeps logical time. Decided to pursue November 9,
  2026, after the `Runtime` rename.

- [ ] **Text reads for File capabilities.** ADR-0022 gives the write side
  `append_text` and `replace_text`, but `File::read(&file)` returns only `Bytes`,
  so a Component reading a text file decodes inside `update` and the
  shortest spelling is `.expect(..)`, a Program-wide fault on bad bytes.
  Pulse's journal did exactly that. A `read_text()` whose outcome carries a
  typed decode failure beside `FsError` would make the correct handling the
  easy one. Found November 2026 reviewing the merged journal.

- [ ] **Finish ADR-0028 (a capability belongs to one Component).**
  Decided November 15, 2026: registration creates capabilities, nothing is
  shared, the installed handle carries them for the host, and plain effects
  are bound per capability. The host-facing half landed beside the old API
  (`ProgramBuilder::new()`, bare `finish()`, `InstalledComponent`,
  `capabilities()` on both handles, per-capability effect Drivers). The
  cut-over is listed under "Implementation status" in the ADR: remove the
  up-front path and `with(&capabilities)`, change `Assembly::install`,
  per-capability type-wide sources, the ownership fault, and migrate every
  usage. The items under ADR-0027 below about type-wide
  binding and the unnamed missing-Driver error are resolved by it.

- [ ] **After ADR-0027 (Drivers supplied per capability value).**
  Plain `Effect<D>` and type-wide Sources are still bound type-wide: one
  Driver serves every capability of that descriptor type and a second is a
  duplicate error, so two clocks cannot have different Drivers. Binding by
  value makes exact per-capability binding the natural rule for them too.
  The missing-Driver error names the descriptor type only; a capability has
  no name or position to report. Named requirement structs (a macro) would
  restore compile-time completeness and are compatible with this.

- [ ] **Unused declared capabilities go unnoticed.** A capability listed in
  `Requires` must be driven by every host even if `update` never touches it;
  pulse's journal requires a clock it binds as `_clock`. Only the
  unused-variable lint could say so, and the underscore silences it. Likely
  acceptable; recorded so `Requires` drifting ahead of use is a known shape.
  Also from the journal: answering a request from an effect result costs two
  private Message variants to carry the reply token, more evidence for the
  composition item below.

- [ ] **Leftover spellings after ADR-0023.** The audit accepted the
  `Kind::verb` entry points and the four finishers, and ADR-0023 now records
  them. Inconsistencies that remain: `PortHandle::request_to` and `request_to_timeout` keep the old
  static shape on the host side (an awaitable builder would match
  `Command::request(&target, ctor).timeout(d)`); `HttpResponsePipeline` still spells
  its mapper finisher `into_command_with`; `Subscription::source_with` is
  the last `_with` pair; and there is no unit-test inspector for a
  `finish_with` continuation, only controlled execution. The reply-token
  cost recorded above is resolved by `finish_with`; pulse's journal uses it.

- [ ] **Constructor-function derive.** ADR-0016 left one sending API, with
  `M::Variant { .. }` spelled at every call site. If that proves noisy once
  the examples have lived with it, a small derive that emits `pub fn
  variant(args) -> Self` constructors (and `fn request(args, reply_with)` for
  reply-token variants) is the remedy; it must not reintroduce a request
  trait or a second Command form.

- [ ] **Single-Component Assembly shortcut (wait for a pattern).** Pulse's
  Journal has a three-line `install`: register one Component, pass every
  capability through, export `accepting(map)`. That is the degenerate case of
  a general mechanism, not evidence the mechanism is too strong; `install`
  earns its shape when an Assembly splits authority, wires its own Ports, or
  chooses its front door. If several single-Component Assemblies appear, the
  shortcut must generate the `install` body (macro or derive). A
  `SimpleAssembly` trait with `type Component` does not work: an associated
  type in a public impl must be public (E0446), which forces the Component
  and then its `Message` to be `pub` and undoes the privacy the Assembly
  provides. A function body is where private types can be named freely.

- [ ] **No small-scale composition: `Command::map` and `Subscriptions::map`
  are missing.** Nothing lifts a `Command<Child>` into a `Command<Parent>` or
  a `Subscriptions<Child>` into a `Subscriptions<Parent>`; the existing `map_*`
  methods are test inspectors. Without Elm's `Cmd.map` and `Sub.map`, every
  reusable piece of stateful logic, however small, must become a Component
  with a Port, a `From` impl, and a message enum. The pulse journal raised it:
  on the merits it is roughly 60/40 an Assembly over an embedded
  sub-reducer, but the question could not be decided because only one shape
  exists. Both maps are implementable from what Commands already carry.
  Specify trace attribution for a lifted Command and document the
  pure-sub-reducer pattern before implementing. Decision for pulse, November
  7, 2026: the journal stays an Assembly as the test bed for that design,
  not because the sub-component shape was weighed and rejected.

- [ ] **First-party filesystem effects.** Governing principle: if a sequence
  must fail together, it is one effect. Owned and Shared handles, a bound root
  for runtime-named files, the five file uses the capability model forces us
  to name, the dispatch-order ADR prerequisite for ordered sinks, lifecycle
  under Drain and Cancel, and sessions as a separate concept are all worked
  through in the [filesystem design note](filesystem-design.md). A provisional
  `fs` module landed November 8, 2026 (ADR-0013); the ordering ADR and the
  handle questions remain the open design work.

## Component Completion and Supervision — Open Design Questions

Positions reached November 2026, not yet an ADR:

- **Shutdown is never a reason on an effect outcome.** Drain lets in-flight
  effects finish with real outcomes; Cancel aborts them and drops their
  mappers. Delivering `Cancelled(Shutdown)` would mean running `update`,
  whose Commands either run (so it was a drain) or are discarded (so the
  transition was unobservable). "The program is stopping" is a fact about
  the Component, not about each effect it has in flight.
- **Stopping is a `Component::shutdown` method, not a Message.** Landed as
  [ADR-0024](../adr/0024-component-shutdown.md); the notes below are the
  record of how it was decided. Sketch:
  `fn shutdown(config, capabilities, model: Self::Model) -> Command<NoMessage>`
  with a default of `Command::none()`. No Message variant to invent, nothing
  another Component can forge, the Model taken by value so held reply tokens
  can be answered, and termination guaranteed by the return type: finishing
  work only (ADR-0023). Its semantics, decided November 2026 and ready for
  an ADR:
  1. **Modes.** Only Drain calls it. Cancel still means stop without
     reacting.
  2. **Timing.** After quiescence: Drain runs as today until nothing is
     pending, so `shutdown` sees a Model that has absorbed every outcome.
  3. **Order.** Reverse registration, one Component at a time, so dependents
     stop while what they depend on is still alive. Deterministic under
     control.
  4. **Late work.** A finishing send or reply to a still-live Component is
     delivered and that Component's Commands run normally; the runtime
     returns to quiescence before stopping the next one.
  5. **After a Component has stopped.** Sends and outcomes addressed to it
     are dropped and traced. A request to it completes at once for the
     requester with a typed outcome saying the provider has stopped.
  6. **Bounding.** Finishing effects are awaited like drained work; the
     ADR-0010 escalation deadline is the only bound. No new setting.
  7. **Faults.** A faulted runtime calls no `shutdown`.
  8. **Escalation.** If Drain escalates to Cancel partway, Components not yet
     stopped are cancelled without the call, in-flight finishing effects are
     aborted, and the shutdown report names the Components that did not get
     their step.
  Consequences to write up: item 5 needs a produced `RequestOutcome` or
  `RequestError` variant (`ReplyAbandoned` or a provider-stopped reason are
  reserved and unproduced today); item 8 extends `ShutdownReport`; the trace
  needs a shutdown event per Component.
- **Cancellation should appear in an outcome type only where something that
  cancels is present.** Landed as [ADR-0025](../adr/0025-bare-effect-outcomes.md);
  what follows is the reasoning as recorded at the time. The live runtime has no source of per-effect
  cancellation today; `EffectFailure::Cancelled` reaches a Component only
  from a controlled test. A bare effect's outcome could be success or its
  Driver's error, making `|EffectOutcome::Succeeded(t)| ..` irrefutable for
  an infallible effect such as `ReadSystemTime`, with a deadline or
  supersession step adding its reason to the type when asked for. This would
  undo part of ADR-0020, which is acceptable; decide it when the first
  mechanism that cancels an effect is designed.

- **Completion rule:** Consider having `Command::stop(...)` declare that its
  Component is finished, with the runtime finishing when all Components have
  stopped. Define how runtime-owned cleanup and faults reach the host. A
  single-Component CLI should not need a Supervisor just to finish on EOF.
- **Meaning of stopping:** Decide whether "Stopping" is runtime state or an
  application Model state used while cooperating with shutdown. Define the
  point after which no further transitions run, and what happens to accepted
  messages, effects, timers, subscriptions, and output at that point. Do not
  assume that requesting a stop and completing cleanup are the same event.
- **Delivery after stop:** Consider an enum selecting error or drop for ordinary
  messages sent to a stopped Component. Define who observes an error for both
  host sends and Component commands. Requests requiring replies must fail
  explicitly; cover both new requests and accepted but unanswered requests,
  along with late replies and results from the stopped Component's own work.
- **Cooperative shutdown:** Explore a library Supervisor using ordinary messages
  to ask Components to finish, coordinate dependencies and acknowledgements,
  and apply grace periods through runtime-owned timers. Define reliable
  completion observation after cleanup, including how the Supervisor learns
  that its children have stopped and when it can stop itself.
- **Authority and escalation:** Decide how program assembly grants a Supervisor
  authority over children, whether it can force them to stop, and how this
  interacts with host Drain/Cancel and runtime faults. Avoid putting shutdown
  ordering or deadline policy into the runtime.
- **Restart:** Supervision also raises restart semantics. Define which failures
  are recoverable, what state and capabilities survive, how re-initialization
  works, and how cleanup completes before replacement. Address identity and
  generations, stale messages/results, outstanding requests, and bindings that
  cannot be recreated. Keep restart strategy and backoff policy in the library
  Component; relate the necessary mechanisms to Subscription restart revisions.
- **Acceptance evidence:** Require an ADR and matching live and controlled tests
  for EOF with outstanding checks/output, multi-Component completion,
  cooperative shutdown, delivery/request behavior after stop, and any adopted
  restart or escalation behavior. Keep the remaining design details open rather
  than treating this list as a complete contract.

The lifecycle proposals require ADRs and matching acceptance evidence before
changing observable semantics. Existing contracts are described in
[subscription reconciliation and Source bridges](../api-guidance.md),
[host lifecycle](../adr/0007-live-host-lifecycle.md), and
[shutdown escalation](../adr/0010-shutdown-escalation.md).
