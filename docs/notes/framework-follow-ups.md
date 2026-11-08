# Framework Follow-Ups

Exploratory suggestions, September 22, 2026. Observable semantic changes need
an ADR before implementation. Shutdown escalation is complete under
[ADR-0010](../adr/0010-shutdown-escalation.md).

Teaching principles for the future guide live in [User Guide Notes](user-guide-notes.md).

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

## URL Checker API Exercise — September 23, 2026

Open items from writing the application by hand; these are exploratory proposals,
not accepted API decisions or a review of the unfinished application.

- [x] **Example-led rustdoc:** Make Source construction discoverable without
  navigating several types. Show a small, complete path from declaring a Program
  capability, storing it on a Component, and returning a Subscription to binding
  its live implementation. Include the controlled equivalent and handling for
  `SourceEvent::Item`, `Ended`, and `Failed`. Link the recipe from the relevant
  constructors and traits, and compile-check the examples as doctests. Completed
  September 24, 2026 in [`src/lib.rs`](../../src/lib.rs): public API descriptions
  now lead with caller behavior, with complete source wiring, request/reply,
  HTTP, driver, shutdown, and controlled-test examples. Existing compile-fail
  checks remain covered; no runtime behavior changed.
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
- [ ] **Composable Programs / subprograms:** Explore `urlchecker::program()` as
  an application library's public assembly boundary, shared by its executable
  and controlled tests. Let callers use deliberately exposed handles,
  capabilities, and protocols without knowing the internal wiring. Extend that
  idea to composing assemblies into a larger Program while keeping internal
  Components, timers, and capabilities private where appropriate.
  Separate assembly from execution: a subprogram could contribute Components
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
  LiveRuntime and ControlledRuntime both accept a Program without sharing a
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

- [ ] **Time access beyond delayed delivery:** Logical time currently reaches
  Components only through `Command::after`. That covers scheduling but not
  observation: a Component cannot learn the current time to stamp a journal
  entry, compute an age, or compare against a deadline without reading the wall
  clock inside `update`, which the purity rule forbids. The workaround is an
  application-defined `Now` effect with its own Driver, which is ceremony for
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

- [x] **Library assembly via `install`:** `examples/pulse` assembles its journal
  through `journal::install(&mut ProgramBuilder) -> Port<Journal>`. The module
  declares its Component, Port, and capabilities, binds the Port, and exports
  only the Port; the Component type, its `ComponentId`, and its capabilities
  stay private. This is a working, runtime-free answer to the composable
  Programs item above and should become the documented shape for library
  Components. Three tensions came up while designing it:
  - **Driver ownership.** `install` must not bind live Drivers. Assembly is
    shared by live and controlled execution, and binding is an
    execution-profile decision, so a Driver argument to `install` would tie it
    to one profile. The caller therefore has to bind a Driver for every
    capability `install` declared, and today learns which ones only from the
    synchronous failure in `LiveRuntime::builder(program).build()`. That
    backstop is correct but late and far from `install`.
  - **Making requirements visible.** Candidate A returns a `#[must_use]` token
    alongside the Port, with `live(runtime, drivers...)` and
    `controlled(runtime)` methods that discharge it; requirements show in the
    type and the binding is typed, but `let _ =` defeats the lint and the
    builder stays the real guarantee. Candidate B puts the requirements in the
    `install` signature as parameters, so they cannot be skipped. B needs two
    things the runtime lacks: a `Binding<D>` abstraction implemented by live
    Drivers and by a controlled marker, because controlled execution has no
    Driver and must never run one; and a single builder that spans assembly
    and binding, because `install` writes into `ProgramBuilder` while profile
    builders only exist after `program.build()`. B would also revise the
    earlier direction that live and controlled runtimes share no interface,
    and it touches the two-stage validation in ADR-0008, so it needs an ADR.
    Use A in `pulse`; its ergonomics are the evidence for whether B is worth
    the staging change.
  - **`install` as a Component trait member.** Consider an associated
    function such as `fn install(program: &mut ProgramBuilder, config: Self::Config)
    -> Self::Exports`, giving every Component one discoverable entry point and
    enabling a `program.install::<FsJournal>(config)` helper. It must be an
    associated function, not a method, because it constructs the Component
    from capabilities the builder issues. Open questions: `Exports` must carry
    the Port plus any binding requirements while keeping `ComponentRef` and
    capabilities private, yet the url checker's tests wanted the
    `ComponentRef` retained privately; identity must come through `Config`
    for multiple instances; and a one-function-per-Component shape does not
    cover a module that installs several cooperating Components, which is the
    subprogram case the item above cares about. Decide whether the trait
    member is the primitive and module-level `install` composes it, or whether
    the free function stays the idiom and the trait only standardizes naming.
  Gather evidence from `pulse` before choosing: how much `main` and the
  controlled tests have to know about the journal's internals, whether the
  token is ever discharged with `let _ =`, and whether a second installed
  module repeats the same ceremony. Landed November 7, 2026 as the
  `Subprogram` trait with profile-generic `Drivers` and
  `with_subprogram` on both builders; see the
  [Subprogram design note](subprogram-design.md). Requirements as `install`
  parameters was considered and rejected because it would merge assembly and
  binding into one builder and erase the Program/Runtime divide. Still open
  from it: identity scoping through `Config`, and a Port-level
  controlled ingress (`notify`/`request` on `ControlledRuntime` keyed by
  `Port`, with replies observed through a token after `run_until_idle`) so a
  Subprogram can be tested from outside its module; exporting a `ComponentRef`
  does not work for that because it makes the Component type public.

- [ ] **No small-scale composition: `Command::map` and `Subscriptions::map`
  are missing.** The pulse journal raised the question of whether it should be
  a Subprogram (a peer Component with its own identity, serialization,
  capabilities, and Port protocol) or an Elm-style sub-component (a pure
  sub-reducer embedded in the parent's Model, called from the parent's
  `update`, with its Commands lifted into the parent's Message). On the
  merits the journal is roughly 60/40 toward Subprogram: it owns two effects
  and a hold timer and would be reused by another application, but in pulse it
  has one consumer, `history` is an in-process lookup dressed as
  request/reply, and a transition costs an extra hop and a second trace
  identity for one logical state change. The question could not be decided on
  the merits because Samara offers only one shape. Nothing can lift a
  `Command<Child>` into a `Command<Parent>` or a `Subscriptions<Child>` into a
  `Subscriptions<Parent>`; the existing `map_*` methods are test inspectors.
  Without the equivalent of Elm's `Cmd.map` and `Sub.map`, every reusable
  piece of stateful logic, however small, must become a Component with a Port,
  a `From` impl, and a protocol. That is a larger DX gap than the controlled
  Port ingress above. Both are implementable: effect Commands already carry a
  boxed outcome mapper, timers carry a Message, replies carry none, and
  batches map elementwise; Subscriptions compose their event mappers the same
  way. Specify how a lifted Command's trace attribution and the
  pure-sub-reducer pattern (capabilities passed in by the parent, child state
  inside the parent Model) are documented before implementation. Decision for
  pulse, November 7, 2026: the journal stays a Subprogram, because it is the
  test bed for that design and genuinely has its own effects and timer, not
  because the sub-component shape was weighed and rejected.

## Component Completion and Supervision — Open Design Questions

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
