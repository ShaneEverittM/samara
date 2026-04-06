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

### L3: Runtime Loop Integration Tests
- Validate mailbox ingestion, update execution, command dispatch, and message re-entry.
- Confirm no bypass of message flow.

### L4: Cancellation and Shutdown Tests
- Validate graceful shutdown with in-flight tasks.
- Confirm expected message delivery guarantees during shutdown policy enforcement.

### L5: Backpressure and Load Tests
- Validate queue pressure behavior and runtime stability.
- Produce metrics for Option A operating envelopes and migration trigger checks.

### L6: Simulation-Time and Acceleration Tests
- Validate that runtime scheduling semantics can run in simulated time.
- Validate faster-than-real-time execution paths for simulation workloads.
- Confirm determinism across repeated accelerated runs with identical inputs.

### L7: Topology Reconsideration Validation Suite
- Run only when proposing a topology change beyond Option A.
- Capture determinism, throughput, and fault isolation findings for superseding ADR proposals.

## Required Scenario Coverage
- Deterministic transitions for representative domain message sets.
- Command correctness for happy path and failure path transitions.
- Adapter/protocol translation correctness (`AppCmd <-> system operations <-> Msg`).
- Runtime fault conversion into explicit `Msg` variants.
- Actor interaction coverage for `tell` (fire-and-forget) and `ask` (request/reply), including dropped-reply failure paths.
- Typed request coverage for `Message<Actor>` mappings and `tell_request` / `ask_request` runtime flows.
- Deferred-reply coverage where commands/messages do not carry explicit reply fields and runtime metadata is used instead.
- Port/protocol binding coverage for provider swapping (`real` vs `mock`) without consumer code changes.
- Port call coverage (`ask`/`tell`) for context-provided reply tokens, detached tell replies, and runtime-owned reply resolution.
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
- Must conform to ADR-0001 Option A semantics.
- Any change away from Option A requires a superseding ADR and L7 evidence.

## PR Evidence Requirements
- Invariant impact summary.
- Failure mode analysis.
- Rollback/recovery notes.
- Test matrix indicating covered layers and gaps.

## Notes for v0
- Persistence/replay tests are explicitly excluded from v0 scope.
- This document is normative for implementation planning and PR review.
