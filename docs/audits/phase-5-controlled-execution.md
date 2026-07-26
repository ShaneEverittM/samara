# Phase 5 Audit: Controlled Execution

- Status: Accepted by Shane
- Date: July 23, 2026
- Governing phase: `docs/goal-mode-checklist.md`, Phase 5
- Baseline: commit `8158f1b`

> Historical note: ADR-0008 later added Program-issued Effect and Source
> capabilities and complete profile-build validation. API spellings and the
> open-dependency qualification recorded in this accepted phase snapshot are
> intentionally preserved as historical evidence, not current guidance.

## Outcome

The bounded Phase 5 controlled runtime is implemented. One topology-neutral
`Program` can now be assembled into deterministic, synchronously driven
controlled execution. The runtime interprets typed Commands, routes Messages
through Component transitions, reconciles runtime-owned Sources, lowers
composed SourceDescriptors through ordered Layers, intercepts effects, advances
logical time, completes successful Requests, reports semantic pending work, and
collects an always-on structural causal trace.

No live Driver, Tokio I/O, wall-clock sleep, or detached task is reachable from
the controlled path. Live execution remains the Phase 6 boundary. This phase
does not change ADR-0002, ADR-0003, the vision, glossary, API guidance, API
contract, acceptance matrix, or test strategy.

## Implemented Slice

### Fallible program assembly and typed routing

- `ProgramBuilder::build()` rejects duplicate Component identities, duplicate
  `(Protocol type, PortId)` declarations, Ports without exactly one binding,
  foreign Ports, and foreign or unregistered providers.
- Port cycles remain legal. The builder does not introspect Component fields or
  behavior-dependent direct sends.
- `Port` and `ComponentRef` retain private Program provenance. Direct sends,
  notifications, Requests, replies, controlled inputs, state inspection, and
  pending effects validate that provenance and their erased Rust types before
  use.
- Every delivered value re-enters the target through its ordinary typed
  Component Message transition. Async or controlled mechanisms never mutate a
  Model directly.

### Controlled effects and successful Requests

- `control_effect::<E>()` authorizes interception of concrete terminal effect
  type `E`; `next_effect::<E>()` moves out the original descriptor without
  cloning it.
- `complete()` accepts one typed `EffectOutcome`, records it as a parentless
  controlled-input root correlated to its originating Command occurrence,
  invokes the one-shot mapper once, and queues the resulting Message.
- Missing controlled effect behavior faults at the emitted Command boundary
  before either a mapper or live Driver can run.
- Request interpretation creates one opaque runtime correlation token and one
  outstanding-Request obligation. A typed `Command::reply` resolves that exact
  occurrence at most once, maps `RequestOutcome::Replied`, and preserves the
  Request-to-provider-to-reply-to-requester causal chain.
- Unanswered Requests remain pending until controlled cancellation. Phase 5
  does not synthesize request failure, timeout, or cancellation outcomes.

### Source realization, composition, and cutover

- Reconciliation realizes one Source per `(ComponentId, SubscriptionId)` and
  classifies start, retain, replace, and cancel decisions after every committed
  transition.
- Equal descriptors preserve the same Source generation and Layer state while
  atomically adopting the newest mapper. Messages already created retain their
  original meaning.
- Replacement allocates a private generation. Old raw events and already-mapped
  Messages are checked independently, discarded before transition, and traced.
- A partially sealed, crate-internal dispatch hook lowers ordinary descriptors
  as terminal and lets built-in `Framed` descriptors recursively append the
  same stateful Layers in controlled execution that live execution will use.
  Its token and SourcePlan result are crate-private, so downstream
  SourceDescriptor implementations can inherit terminal behavior but cannot
  override general lowering.
- Only the terminal descriptor is controlled. Registering the composed type
  does not conceal a missing terminal binding.
- SourcePlan event-type chains are validated at activation. Internal unit tests
  deliberately construct malformed crate-owned plans and prove they are
  rejected before reaching a type-erased Layer or Subscription downcast.
- A terminal failure or normal end stops that Source generation after its typed
  event is mapped. The accepted Decoder EOF limitation remains unchanged.

### Scheduling, trace, and ownership

- Controlled work is ordered by `(logical deadline, insertion ticket)`.
  Initialization is canonicalized by `ComponentId`; Command traversal and
  harness input order allocate deterministic tickets.
- Manual advancement first drains due-now causes, then visits every reachable
  intermediate deadline up to the target. Automatic advancement uses the same
  causal schedule. Logical-time overflow is reported rather than panicking.
- Each trace record has a deterministic run-local identity and logical time.
  Initialization, controlled Message/Source inputs, and scripted effect
  outcomes are parentless roots; every other record has exactly one earlier
  immediate cause.
- Trace events explain Component transitions, Command kinds and targets,
  Subscription lifecycle, Source mapping and stale drops, effect and Request
  outcomes, timers, and runtime faults without copying application payloads.
- `pending_now` counts accepted Component Messages and due timers.
  `pending_later` counts pending effects, future timers, active Sources, and
  outstanding Requests. Internal source-event work and trace records are not
  additional semantic obligations.
- Controlled cancellation consumes the runtime, cancels every remaining
  obligation, and returns zero remaining, pending-now, and pending-later work.

## Acceptance Evidence

| Requirement | Phase 5 evidence |
| --- | --- |
| Program validation | Five named `phase5_program_build_*` tests cover every ADR-0003 error class and legal Port cycles. |
| V2 Message-only interaction | `v2_cross_component_interaction_reenters_as_message` follows a direct send into a target transition and causal trace edge. |
| V3 controlled effects | `v3_controlled_effect_maps_exactly_once_without_live_driver` inspects the owned typed descriptor, supplies one outcome, verifies one mapper call, and proves the mapped Message transition. Missing behavior tests fault before mapping. |
| V4 Source lifecycle | Retained-newest-mapper, already-created Message, raw stale event, mapped stale Message, terminal-only control, nested Layer lowering, and internal malformed-plan invariant tests exercise the complete controlled Source contract. |
| V6 deterministic equivalence | Repeated runs compare both all Component state and the complete structural trace; a separate test rejects equal state with a different trace. |
| V7 logical time | Equal-time declaration order, ComponentId initialization, harness order, manual/automatic equivalence, due-cause draining, and overflow diagnostics are active. |
| V8 causality | Direct sends and successful Requests assert explicit trace edges without naming a mailbox, task, worker, or live total order. |
| V9 structured ownership | Tests separately observe due Messages/timers, effects, future timers, Sources, Requests, and zero-work cancellation. |
| V10 non-influential trace | Trace reads are immutable; root and non-root causation, logical time, and stale-generation diagnostics are executable assertions. |
| V11 onboarding | `examples/minimal.rs` runs its declared stream through controlled execution; both larger reference Components also run end to end. |

## Public API Scope Audit

- `SourcePlan` is crate-private and absent from the crate root, prelude, and
  generated public documentation.
- The hidden SourceDescriptor dispatch method has a default terminal
  implementation, but both its token and result are crate-private. This is a
  partially sealed method: downstream types can implement SourceDescriptor but
  cannot name the signature required to call or override lowering.
- A compile-fail doctest exercises an attempted downstream override. Ordinary
  downstream SourceDescriptor implementations continue compiling in the
  integration tests and examples.
- The only production override is Samara's built-in `Framed` descriptor.
  Malformed-plan tests live inside the crate and therefore do not promise a
  public Layer or SourcePlan construction seam.
- The application-facing shape remains
  `Subscription::source(id, Framed::new(source, decoder), mapper)`, with only
  the terminal source type controlled.

## Topology and Ordering Audit

The implementation uses one owned synchronous scheduler as a small controlled
mechanism. No public API, Component, acceptance assertion, or trace event names
that mechanism as a mailbox, actor task, central event loop, worker, or global
live order. The only controlled total-order commitment is ADR-0003's logical
deadline plus deterministic insertion ticket.

Transitions remain serialized and non-overlapping because the controlled core
owns each Component kernel exclusively. Cross-Component sends and Port delivery
are scheduled Messages, never nested Component calls. The Phase 3 optional
Tokio mutex remains available for a future concurrent live topology and is not
used as controlled scheduling evidence.

## Invariant Impact

- `Model` remains runtime-owned and has no controlled mutation path except a
  typed `Message` transition.
- Command, EffectDescriptor, SourceDescriptor, and Subscription values remain
  inert declarations until the controlled interpreter accepts them.
- Controlled effects and terminal source inputs are supplied as typed data;
  no live Driver fallback exists.
- Layer state is runtime-owned and profile-independent. Source generation is
  private runtime metadata, not domain state.
- Runtime mechanism contains no retry, reconnect, framing selection,
  notification-failure, or Request-timeout policy.
- Determinism covers conforming Component, mapper, decoder, and descriptor code.
  User code that reads ambient state or performs hidden side effects remains a
  documented conformance violation rather than something type erasure can
  prevent.

## Failure Modes and Recovery

- **Missing controlled terminal behavior:** the runtime records the Component,
  work occurrence, and terminal descriptor type, then becomes faulted. Models
  and trace remain readable; driving cannot resume; cancellation remains
  available. Recovery is to correct controls and create a fresh run.
- **Malformed internal SourcePlan:** activation validates the complete
  event-type chain and faults before accepting Source input. No mapper or Layer
  downcast runs. Downstream code cannot construct or override such a plan.
- **Foreign runtime value:** a ComponentRef, Port, or PendingEffect from another
  Program/runtime is rejected as a harness or runtime diagnostic.
- **Absent direct-send target or invalid reply:** interpretation records a
  runtime fault with its Command occurrence. No alternate route is attempted.
- **Duplicate desired Subscription identity:** reconciliation faults before
  silently selecting one desire.
- **Logical-time overflow:** harness advancement returns a harness error without
  faulting an otherwise usable run; a Component-emitted impossible timer
  deadline records a runtime fault.
- **Non-conforming user code:** Components, mappers, descriptors, and decoders
  can still panic or perform hidden effects. Phase 5 adds no panic-containment
  or sandbox promise.

Rollback is source-only: revert the controlled core, the type-erased
interpreter seams, activated acceptance tests and reference tests, status prose,
and this audit packet. Controlled execution owns no external resource or
persistent state requiring recovery.

## Preserved Deferrals

- Live Tokio scheduling, EffectDrivers, SourceDrivers, `mpsc`/TCP bridges,
  backpressure, overload, and structured live shutdown remain Phase 6.
- Request failure, timeout, abandonment, late reply, delegation, and in-band
  cancellation remain deferred.
- Notification delivery-failure policy remains deferred.
- General public Layer/profile-binding abstractions remain open. Phase 5 uses a
  partially sealed internal stable-Rust dispatch hook for built-in composition;
  it exposes neither SourcePlan nor downstream lowering.
- Domain-payload trace capture, typed projections, streaming observers, durable
  storage, replay, and persistence remain out of scope.
- Adoption of `bytes` and Decoder EOF/finalization semantics remains required
  before live TCP framing. Phase 5 preserves the Phase 4 behavior exactly.

## Validation

| Gate | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Pass |
| `cargo test --all-targets` | Pass: 79 tests, 0 ignored (35 Phase 5 integration and 2 internal SourcePlan invariants) |
| `cargo test --doc` | Pass: 1 runnable doctest and 8 compile-fail contracts |
| `cargo clippy --all-targets --all-features -- -D warnings` | Pass |
| `RUSTDOCFLAGS='-D warnings' cargo doc --no-deps --all-features` | Pass |
| `git diff --check` | Pass |
| Phase-boundary scope scan | Pass: no live Driver invocation, Tokio I/O, async controlled path, wall sleep, durable replay, or governing-contract change |
| Public API scope scan | Pass: no public SourcePlan item or callable/overridable lowering method; attempted downstream override is compile-fail while ordinary custom terminal descriptors compile |
| Independent read-only audit | GO: external implementation and override probes confirm the hook is partially sealed; public rustdoc exports no SourcePlan or lowering method; built-in nested lowering remains green |

## Manual Spot Check

Review these in order:

1. `ControlledCore::initialize`, `run_until_idle`, `advance`, and
   `advance_to_next` in `src/controlled_runtime.rs` — confirm the deterministic
   key is controlled-only machinery and due causes run before time advances.
2. `ControlledCore::interpret_command` — confirm every Command becomes typed
   runtime-owned work or an explicit fault and no branch invokes a Driver.
3. `apply_subscription_changes` and `process_source_event` — confirm retained
   mapper replacement, hard generation cutover, ordered Layer mapping, and both
   stale-work checks.
4. The partially sealed SourceDescriptor dispatch and `Framed` implementation
   in `src/lib.rs` — confirm applications declare one composed descriptor while
   neither SourcePlan nor general lowering is downstream-extensible and only
   the terminal boundary is controlled.
5. `TraceRecord`, `TraceEvent`, and effect completion — confirm harness inputs
   are roots, non-roots have one immediate parent, and effect correlation does
   not pretend the harness outcome was caused by application code.
6. `pending_work` and `cancel` — confirm counts use ADR-0003 semantic units and
   reach zero without manufacturing application outcomes.
7. `tests/phase5_controlled_execution.rs` — confirm assertions remain
   topology-neutral and directly cover every required ADR-0003 scenario.
8. The controlled tests in all three examples — confirm the same Components and
   program factories remain readable without controlled-only application logic.
9. The preserved Decoder behavior — confirm Phase 5 did not select EOF or
   `bytes` semantics ahead of Phase 6.

Shane accepted this audit on July 23, 2026. Phase 5 is closed. Shane subsequently
accepted ADR-0004 and authorized Phase 6 implementation.
