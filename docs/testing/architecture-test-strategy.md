# Architecture Test Strategy (v0)

## Status
- Phase: Documentation-first.
- Date: February 28, 2026.

## Purpose
Define required test layers and acceptance gates for a strict TEA + Tokio architecture before implementation begins.

## Test Layers

### L0: Update Determinism Tests
- Verify `update(model, msg)` is deterministic and side-effect free.
- Same input must yield identical `(next_model, cmds)`.

### L1: Command Emission Tests
- Validate message-to-command mapping.
- Ensure command intent is explicit and complete for each transition.

### L2: Effect Handler Contract Tests
- Validate each `Cmd` variant maps to expected async behavior.
- Confirm handler failures are mapped to structured runtime error messages.
- Validate adapter/protocol composition boundaries (mechanism vs policy split).

### L3: Runtime Message-Flow Integration Tests
- Validate message ingestion, target delivery, update execution, command dispatch, and message re-entry.
- Confirm that transitions for one Component never overlap.
- Confirm required causal edges without assuming an order between independent live events.
- Confirm no bypass of message flow.

### L4: Cancellation and Shutdown Tests
- Validate graceful shutdown with in-flight tasks.
- Confirm expected message delivery guarantees during shutdown policy enforcement.

### L5: Backpressure and Load Tests
- Validate queue pressure behavior and runtime stability.
- Produce throughput, tail-latency, isolation, and overload evidence for the selected implementation.

### L6: Simulation-Time and Acceleration Tests
- Validate that runtime scheduling semantics can run in simulated time.
- Validate faster-than-real-time execution paths for simulation workloads.
- Confirm determinism across repeated accelerated runs with identical inputs.

### L7: Topology-Independent Conformance Suite
- Validate per-Component serialization and required causal relationships.
- Confirm application code and conformance tests do not rely on incidental scheduler topology.
- Run before and after topology-coupled implementation changes and compare observable traces.

## Required Scenario Coverage
- Deterministic transitions for representative domain message sets.
- Command correctness for happy path and failure path transitions.
- Adapter/protocol translation correctness (`AppCmd <-> system operations <-> Msg`).
- Runtime fault conversion into explicit `Msg` variants.
- Component interaction coverage for `Cmd::notify` (one-way) and
  `Cmd::request` (request/reply), including dropped-reply failure paths.
- Typed request coverage for `Request<P>` associated Reply mappings and
  `RequestOutcome` runtime flows.
- Deferred-reply coverage where provider messages carry an inert `ReplyTo` and
  transitions emit `Cmd::reply` rather than using a live reply channel.
- Port/protocol binding coverage for provider swapping (`real` vs `mock`) without consumer code changes.
- Port interaction coverage for the `Notification<P>` / `Cmd::notify` and
  `Request<P>` / `Cmd::request` symmetry, opaque correlation, result mapping,
  and runtime-owned reply resolution.
- Reply-obligation coverage must demonstrate that:
  - consuming `ReplyTo` into an interpreted `Cmd::reply` produces exactly one
    typed outcome without a diagnostic violation;
  - dropping an unresolved `ReplyTo` is reported with enough Component and
    request context to locate the violation;
  - constructing and then discarding `Cmd::reply` does not falsely discharge
    the obligation;
  - storing or deliberately forgetting `ReplyTo` cannot evade runtime-owned
    detection when the defined request lifecycle ends; and
  - diagnostic `Drop` checks never panic or influence application-visible
    behavior.
- Public API lint checks must verify that discarding a `ReplyTo`-valued
  expression triggers its `#[must_use]` diagnostic without claiming that the
  lint proves eventual consumption.
- Runtime drive-loop tests should prefer `run_until(...)` / `run_until_predicate(...)` / `run_until_idle()` over hard-coded sleep durations.
- Cancellation behavior for long-running and short-running commands.
- Backpressure behavior under burst and sustained load.
- Recovery behavior after handler and runtime errors.
- Simulated-time progression behavior (including faster-than-real-time runs) for timer-driven flows.

## Acceptance Gates by Change Type

### Architecture or Contract Changes
- Must update architecture docs and relevant ADR.
- Must update affected test strategy sections.
- Must include new/updated tests for all impacted layers.

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
