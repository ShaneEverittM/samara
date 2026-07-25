# Architecture Test Strategy (v0)

## Status
- Phase: Phase 6 live-runtime implementation complete; ADR-0006 live Port-ingress evidence active.
- Date: July 25, 2026.

## Purpose
Define required test layers and acceptance gates for the strict TEA + Tokio
architecture before each implementation slice begins.

## Test Layers

### L0: Update Determinism Tests
- Verify transitions are deterministic and side-effect free for fixed immutable
  Component configuration.
- Equivalent configuration + Model + Message input must yield equivalent next
  Model and Command intent.
- Confirm behaviorally relevant mutable state cannot hide in Component
  configuration or bypass the Model.

### L1: Command Emission Tests
- Validate Message-to-Command mapping.
- Ensure command intent is explicit and complete for each transition.
- Inspect typed descriptors without executing live work and exercise pure message
  mappers with equivalent outcomes or events.

### L2: Descriptor, Layer, and Driver Contract Tests
- Validate the public descriptor and Command boundary cannot substitute an
  opaque async closure for an identifiable EffectDescriptor.
- Validate each terminal EffectDescriptor reaches the matching live
  EffectDriver or controlled behavior. Every EffectOutcome actually accepted
  by the running scope completes the obligation exactly once: mapped effects
  invoke their one-shot mapper and emit the expected Component Message, while
  discarded-outcome effects emit no Message.
- Validate normal EffectDriver success and failure produce one outcome, while
  whole-scope Cancel or runtime-fault cleanup aborts the live future
  without manufacturing an outcome or invoking its mapper. Preserve
  the declared mapper-or-discard behavior for explicitly accepted cancellation
  outcomes.
- Validate SourceDescriptor equality has the promised reconciliation meaning
  and each terminal SourceDescriptor reaches the matching live SourceDriver or
  controlled behavior to realize a runtime-owned Source.
- Validate a Source may emit zero or more SourceEvents and that its reusable
  mapper can produce a Component Message for each delivered event.
- Validate successful calls from one Source preserve acceptance order. The
  first accepted fail/end or active silent Driver return produces exactly one
  terminal event; cancellation, replacement, shutdown, or runtime fault
  winning first suppresses terminal mapping and later calls return
  `DriverStopped`.
- Validate retaining an equal SourceDescriptor preserves the Source realization
  while atomically adopting the latest projected mapper.
- Validate replacement is a hard private-generation cutover: stale events or
  mapped Messages cannot begin a transition or cross through the new mapper.
- Validate Layers compose descriptors and outcomes/events identically in live
  and controlled execution without ambient I/O.
- Validate a non-terminal descriptor passes through its Layers before only the
  resulting terminal descriptor reaches a Driver or controlled behavior.
- Validate composed SourceDescriptors automatically lower to SourcePlans and
  applications bind only terminal descriptors.
- Validate Drivers remain terminal, selected by live-profile assembly, and
  runtime-owned.
- Validate normal EOF alone invokes pure Decoder finalization exactly once,
  final frames precede `Ended`, a finalization Error produces failure without
  `Ended`, and no failure/cancellation/replacement path pretends to be EOF.
- Validate mechanism-versus-policy boundaries without requiring a universal
  Layer trait or a particular profile-binding API.

### L3: Runtime Message-Flow Integration Tests
- Validate message ingestion, target delivery, update execution, command dispatch, and message re-entry.
- Confirm that transitions for one Component never overlap.
- Confirm required causal edges without assuming an order between independent live events.
- Confirm no bypass of message flow.
- Confirm Protocol Messages are mapped explicitly into provider Component
  Messages rather than treated as the same vocabulary.
- Confirm successful Requests use opaque runtime correlation, accept at most
  one Reply, map it to `RequestOutcome::Replied`, and preserve the causal chain.
- Confirm live PortHandle Notifications and Requests enter through the exact
  built Port binding and ordinary provider Message path without directly
  invoking a Component or exposing its Model.
- Confirm a host Request uses opaque runtime correlation and `ReplyTo` but
  resolves its waiter to `R::Reply` directly, without a requester Component
  Message or RequestOutcome. Concurrent same-typed requests must remain distinct
  when Replies complete in a different order.
- Confirm `ProgramBuilder::build()` rejects exactly the explicitly knowable
  assembly errors and permits Port cycles without claiming a closed static
  dependency graph.

### L4: Cancellation and Shutdown Tests
- Validate Drain atomically closes ingress and Source admission, stops
  active Sources, and recursively handles pre-cutoff accepted Messages and
  Source deliveries plus causally emitted finite work.
- Confirm Drain does not cancel finite effects to complete and document that a
  hung Driver, unanswered Request, far-future or recurring timer, or
  self-sustaining application may keep it pending indefinitely.
- Validate Cancel closes ingress, stops application driving, cancels
  obligations and owned Driver tasks, invokes no mapper solely because the
  scope ended, and joins or aborts every owned task.
- Validate fault cleanup follows the same ownership closure and surfaces
  `RuntimeError` without fabricated typed application data.
- Confirm successful shutdown reports zero remaining, pending-now, and
  pending-later obligations without depending on exact completed/cancelled
  diagnostic counts.
- Confirm an issued EffectDescriptor still resolves through exactly one
  EffectOutcome when its own contract exposes cancellation and that outcome is
  actually accepted.
- Confirm canceling a Source does not manufacture a SourceEvent unless that
  Source contract explicitly promises one.
- Validate ComponentHandle and PortHandle use one atomic external-ingress
  cutoff. Every shutdown race must classify a Port operation as accepted and
  owned or rejected without delivery.
- Confirm Drain retains admitted Port Notifications and host Requests, including
  their Replies and causal finite work, and cannot report clean completion while
  a host Request remains outstanding.
- Confirm Cancel or non-fault closure wakes a pending host Request with
  `RuntimeError` and no RequestOutcome, while a runtime fault returns the same
  preserved fault surfaced through the owning RuntimeTask.
- Confirm dropping an unpolled host request admits nothing and dropping an
  admitted waiter does not cancel provider delivery, remove the Request
  obligation, or release Drain.
- Confirm `RuntimeTask::run_forever` surfaces terminal runtime faults without a
  prior shutdown request or arbitrary host delay.
- Cancel a pending `run_forever` observation through an ordinary
  `tokio::select!`, then prove the same RuntimeTask still owns usable ingress
  and can perform explicitly selected Drain or Cancel cleanup.

### L5: Backpressure and Load Tests
- Validate live admission and work pressure behavior without assuming a
  particular internal queue topology.
- For v0 unbounded internal delivery, prove a healthy running scope
  does not intentionally drop successfully accepted work in a bounded test.
- Record workload, throughput, tail latency, memory trend, and isolation as
  characterization only. Do not turn capacity or performance values into
  stable conformance thresholds or imply support for memory exhaustion.
- Confirm the first-party `mpsc` bridge adds no pressure promise beyond the
  upstream Tokio channel selected by the application.
- Include admitted PortHandle traffic in the same bounded no-silent-drop
  characterization without promising FIFO among independent handle clones.

### L6: Controlled-Time and Acceleration Tests
- Validate that runtime scheduling semantics can run in controlled time.
- Validate faster-than-real-time execution paths for simulation workloads.
- Confirm determinism across repeated accelerated runs with identical inputs.
- Confirm controlled execution never falls back to a live Driver when terminal
  controlled behavior is unbound. A bound effect, Source, or timer merely
  awaiting future harness input or logical time remains a `pending_later`
  obligation rather than being confused with missing behavior.
- Confirm missing controlled terminal behavior faults at that boundary without
  invoking a message mapper, leaves state and trace inspectable, permits
  cancellation, and prevents resumed driving.
- Confirm equal-time work follows logical deadline plus deterministic insertion
  ticket, including declaration order, harness order, and Component-identity
  ordering rather than registration order.
- Confirm manual and automatic driving report semantic `pending_now` and
  `pending_later` obligations rather than scheduler machinery.

### L7: Topology-Independent Conformance Suite
- Validate per-Component serialization and required causal relationships.
- Confirm application code and conformance tests do not rely on incidental scheduler topology.
- Run before and after topology-coupled implementation changes and compare
  structural semantic traces plus final state. Compare domain descriptor
  payloads in direct typed-intent tests rather than assuming generic trace
  capture.

## Required Scenario Coverage
- Deterministic transitions for representative domain message sets.
- Command correctness for happy path and failure path transitions.
- EffectDescriptor-to-EffectOutcome coverage through both a live EffectDriver
  test double and controlled behavior, including exactly-once message mapping.
- SourceDescriptor-to-SourceEvent coverage through both a live SourceDriver test
  double and controlled behavior, including zero-event, repeated-event, normal
  end, failure, retained-latest-mapper, hard-generation replacement, stale
  drop, and cancellation paths where applicable.
- Live Source terminal-arbitration coverage including explicit end/fail,
  active silent return, terminal-versus-cancellation races, per-Source FIFO,
  later `DriverStopped`, and no hidden restart of a terminal still-desired
  Source.
- Framed EOF coverage including final frames before end, finalization failure
  without end, and no finalization after upstream failure, decoder failure,
  replacement, cancellation, shutdown, or runtime fault.
- Layer composition coverage proving the same descriptor and mapping semantics
  across live and controlled profiles.
- Expected effect, Source, Request, and domain failures whose contracts define
  typed Error data must enter through explicit Component Messages or typed
  boundary outcomes/events where behaviorally relevant.
- Live mechanism faults that cannot truthfully construct typed application
  data, including Driver panic, unavailable dynamic binding, and exhausted
  one-shot `mpsc`, must instead close admission, cancel/join owned work, and
  surface `RuntimeError` through the host boundary.
- Component interaction coverage for `Command::notify` (one-way) and the
  `Command::request` / `Command::request_with` request/reply pair. Phase 5
  activates only the successful Request/Reply path; dropped-reply policy
  remains deferred.
- Typed request coverage for `Request<P>` associated Reply mappings and
  `RequestOutcome` runtime flows.
- Protocol coverage proving `Protocol::Message` is provider-neutral, concrete
  enums follow the `XProtocolMessage` role, and provider
  `Component::Message` values implement the standard `From` conversion used by
  assembly.
- Request delivery coverage where provider Protocol Messages carry a
  `RequestInvocation<P, R>` with inert `ReplyTo<R::Reply>` authority and
  transitions emit `Command::reply` rather than using a live reply channel.
- Port/protocol binding coverage for provider swapping (`real` vs `mock`) without consumer code changes.
- Port interaction coverage for the `Notification<P>` / `Command::notify` and
  `Request<P>` / `Command::request` symmetry, canonical and explicit message
  mapping, opaque correlation, and runtime-owned reply resolution.
- Live PortHandle coverage must prove:
  - exact-Program and exact-binding validation, including rejection of a foreign
    or same-named lookalike Port before spawn;
  - cloned-handle Notification admission, provider conversion, serialized
    delivery, post-cutoff rejection, and cutoff-race ownership;
  - direct typed host Reply with no RequestOutcome or requester Component
    Message, plus correlation across reverse-order concurrent Replies;
  - Drain retention and possible indefinite wait for an unanswered Request;
  - Cancel and clean-closure wakeup through RuntimeError, fault identity
    preservation, and later-ingress rejection with that fault; and
  - unpolled-future no-op and admitted-waiter drop without Request cancellation.
- When the deferred Request lifecycle tranche is activated, Reply-obligation
  coverage must demonstrate that:
  - consuming `ReplyTo` into an interpreted `Command::reply` produces exactly one
    typed outcome without a diagnostic violation;
  - dropping an unresolved `ReplyTo` is reported with enough Component and
    request context to locate the violation;
  - constructing and then discarding `Command::reply` does not falsely discharge
    the obligation;
  - storing or deliberately forgetting `ReplyTo` cannot evade runtime-owned
    detection when the defined request lifecycle ends; and
  - diagnostic `Drop` checks never panic or influence application-visible
    behavior.
- Public API lint checks must verify that discarding a `ReplyTo`-valued
  expression triggers its `#[must_use]` diagnostic without claiming that the
  lint proves eventual consumption.
- Public API lint checks must likewise verify that discarding a `Command`
  triggers its `#[must_use]` diagnostic; a discarded-outcome effect means no
  completion Message, not that the inert Command value itself may be dropped.
- Compile-contract checks must keep `PortHandle` distinct from inert `Port`,
  expose no Model access, and prove it cannot substitute for `Port` in Command
  or ControlledRuntime APIs. Because Rust cannot reject arbitrary fields in a
  `Send` Component configuration, keeping live handles out of Components also
  remains an explicit conformance rule.
- Discarded-outcome effect tests must cover typed controlled interception,
  terminal-outcome tracing without a resulting Message transition, ordinary
  missing-binding faults, live Drain retention, and live Cancel cleanup.
- Runtime drive-loop tests should prefer `run_until(...)` / `run_until_predicate(...)` / `run_until_idle()` over hard-coded sleep durations.
- Controlled trace tests must verify the common record envelope, one immediate
  causal parent for every non-root, roots with no parent, logical time, Source
  stale-drop records, and read-afterward noninterference without requiring a
  streaming callback.
- Controlled equivalence tests compare structural trace and final state;
  direct Component tests separately compare same-typed descriptor payloads.
- Work-accounting tests count accepted Messages and due timers as
  `pending_now`; pending effects, future timers, active Sources, and outstanding
  Requests as `pending_later`; and no internal tasks, queues, locks,
  interpreter steps, or trace records as separate obligations.
- An admitted host Port Request is one outstanding Request obligation; its Tokio
  waiter is not a second work unit. Dropping that waiter does not change the
  count or ownership lifecycle.
- Controlled cancellation tests reduce both pending counts to zero. An
  unanswered Phase 5 Request remains a `pending_later` obligation until then
  and does not synthesize a deferred failure, timeout, or cancellation outcome.
- Cancellation behavior for long-running and short-running commands.
- Standard-continuation tests must cover the default `From` conversion and the
  explicit `_with` mapper for Effects, Requests, Subscriptions, and HTTP
  response pipelines. Evidence must include reusable Source conversion,
  one-shot finite continuations, captured call-site context, and the reflexive
  standard-library `From<T> for T` identity case.
- First-party bridge coverage for one-shot `mpsc` normal closure and explicit
  duplicate/reactivation fault, plus one-connection TCP Bytes, connect/read
  failure, peer EOF, cancellation closure, and absence of hidden
  retry/reconnect/framing.
- First-party HTTP coverage for descriptor fidelity, controlled interception,
  typed configuration and transport failure, duplicate binding, raw
  status/headers/body, redirect non-following, and sequential connection-pool
  reuse against a local server. Generic finite-effect tests continue to own
  Drain, Cancel, mapped and discarded outcomes, and structured lifecycle
  evidence.
- Fluent HTTP response-pipeline coverage for the consuming request/response
  phase boundary, unchanged raw controlled interception, explicit versus absent
  status policy, owned JSON success and failure, retained response/source
  diagnostics, raw failure and cancellation passthrough, at-most-once pure
  transforms, and equivalent Component results for live and controlled raw
  responses. Trace assertions must continue to describe only the raw terminal
  HttpRequest outcome.
- Pressure characterization under burst and sustained load without a stable
  threshold promise.
- Recovery behavior after Driver and runtime failures without prescribing the
  final boundary variant taxonomy. Phase 6 requires scope cleanup and
  host-visible fault, not restart or continued application driving.
- Controlled-time progression behavior (including faster-than-real-time runs)
  for timer-driven flows.

## Acceptance Gates by Change Type

### Architecture or Contract Changes
- Must update architecture docs and relevant ADR.
- Must update affected test strategy sections.
- Must include new/updated tests for all impacted test levels and Layer or Driver roles.

### Runtime Execution Changes
- Must include L3 and L4 coverage.
- Must include failure-path assertions.
- Time-sensitive runtime changes must include L6 coverage.

### Topology-Coupled Changes
- Must conform to ADR-0002's topology-neutral observable semantics.
- Must include L7 evidence and relevant load, lifecycle, and fault-isolation evidence.
- Require an ADR only when public ordering, causality, isolation, or controlled-determinism semantics change.

## PR Evidence Requirements
- Invariant impact summary.
- Failure mode analysis.
- Rollback/recovery notes.
- Test matrix indicating covered layers and gaps.

## Notes for v0
- Persistence/replay tests are explicitly excluded from v0 scope.
- This document is normative for implementation planning and PR review.
