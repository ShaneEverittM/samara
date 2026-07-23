# Architecture Test Strategy (v0)

## Status
- Phase: Phase 5 controlled-execution contract approved.
- Date: July 23, 2026.

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
  EffectDriver or controlled behavior and produces exactly one EffectOutcome.
- Validate the one-shot EffectOutcome message mapper is invoked exactly once and
  emits the expected Component Message.
- Validate SourceDescriptor equality has the promised reconciliation meaning
  and each terminal SourceDescriptor reaches the matching live SourceDriver or
  controlled behavior to realize a runtime-owned Source.
- Validate a Source may emit zero or more SourceEvents and that its reusable
  mapper can produce a Component Message for each delivered event.
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
- Confirm `ProgramBuilder::build()` rejects exactly the explicitly knowable
  assembly errors and permits Port cycles without claiming a closed static
  dependency graph.

### L4: Cancellation and Shutdown Tests
- Validate graceful shutdown with in-flight tasks.
- Confirm expected message delivery guarantees during shutdown policy enforcement.
- Confirm an issued EffectDescriptor still resolves through exactly one
  EffectOutcome when its contract exposes cancellation.
- Confirm canceling a Source does not manufacture a SourceEvent unless that
  Source contract explicitly promises one.

### L5: Backpressure and Load Tests
- Validate ingress and work pressure behavior without assuming a particular
  internal queue topology.
- Produce throughput, tail-latency, isolation, and overload evidence for the selected implementation.

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
- Layer composition coverage proving the same descriptor and mapping semantics
  across live and controlled profiles.
- Runtime fault conversion into explicit Component Messages or typed boundary
  outcomes/events where behaviorally relevant.
- Component interaction coverage for `Command::notify` (one-way) and
  `Command::request` (request/reply). Phase 5 activates only the successful
  Request/Reply path; dropped-reply policy remains deferred.
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
  `Request<P>` / `Command::request` symmetry, opaque correlation, message mapping,
  and runtime-owned reply resolution.
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
- Controlled cancellation tests reduce both pending counts to zero. An
  unanswered Phase 5 Request remains a `pending_later` obligation until then
  and does not synthesize a deferred failure, timeout, or cancellation outcome.
- Cancellation behavior for long-running and short-running commands.
- Backpressure behavior under burst and sustained load.
- Recovery behavior after Driver and runtime failures without prescribing the
  final boundary variant taxonomy.
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
