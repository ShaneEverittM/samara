# ADR 0001: Runtime Topology for Tokio + TEA

- Status: Accepted
- Date: February 28, 2026
- Decision owners: Samara maintainers

## Context
Samara combines strict TEA update semantics with Tokio-based effect execution. The runtime topology determines message ordering guarantees, failure boundaries, throughput characteristics, and testing complexity.

This ADR governs topology-specific implementation work for v0.

## Decision Drivers
- Preserve TEA determinism guarantees.
- Keep side effects isolated behind typed commands and handlers.
- Maintain testability and clear fault handling.
- Support expected load while preserving operational simplicity.
- Preserve a path to simulated-time and faster-than-real-time execution for embedded test scenarios.

## Considered Options

### Option A: Single App Mailbox
- One global queue and update loop.
- Strong global ordering and simpler reasoning.

### Option B: Hybrid Root + Child Loops
- Root loop plus optional subsystem loops.
- Balanced isolation and complexity.

### Option C: Per-Domain Actors
- Multiple autonomous loops with channel-based coordination.
- Highest partitioned scalability and isolation.

## Candidate Ordering Guarantees
- G1: Global total order.
- G2: Per-source FIFO with explicit causal handoff.
- G3: Partitioned/eventual processing.

## Decision
- Select **Option A: Single App Mailbox** for v0.
- Adopt **G1 global total order** message processing semantics.
- Run effect handlers concurrently on Tokio; handler output must re-enter through the single mailbox as `Msg`.
- Keep domain logic and runtime APIs topology-neutral where practical, but runtime behavior must conform to Option A semantics.

## Consequences
- Positive:
  - Highest determinism and easiest reasoning about state transitions.
  - Simplest testing and debugging model for the first implementation phase.
  - Lowest architecture and operational complexity for v0.
  - Single mailbox sequencing aligns well with deterministic simulation playback.
- Negative:
  - State transition throughput is bounded by one update loop.
  - Mailbox contention and backlog risk increase with high command fan-out.
  - Future migration to hybrid/actor topology will require ordering and ownership refactors.
- Mitigations:
  - Treat expensive computations as commands handled asynchronously.
  - Track queue depth, loop latency, and throughput from early integration tests.
  - Keep runtime boundary contracts explicit to reduce migration cost if superseded later.

## Rejected Alternatives
- **Option B: Hybrid Root + Child Loops** (not selected for v0):
  - Better scalability potential than Option A, but introduces higher coordination and testing complexity too early.
- **Option C: Per-Domain Actors** (not selected for v0):
  - Best partitioned throughput and isolation, but highest complexity and weakest global-order story for initial correctness goals.

## Required Evidence (Exit Criteria)
- Correctness:
  - Deterministic state transition tests pass.
  - Command emission tests pass.
- Runtime behavior:
  - Mailbox ingestion/dispatch/re-entry integration tests pass.
  - Cancellation and graceful shutdown tests pass.
  - Runtime errors are surfaced as structured `Msg` variants.
- Performance and resilience:
  - Backpressure/load tests run for candidate topology.
  - Throughput and tail latency evidence captured.
  - Fault isolation behavior validated.
- Maintainability:
  - Complexity impact documented.
  - Test strategy updates merged with decision.

## Implementation Gate
- Runtime implementation must follow Option A semantics until a superseding ADR is accepted.
- Any proposal to move to Option B or C must include:
  - A superseding ADR.
  - Updated ordering guarantees.
  - Migration and compatibility strategy.
  - Comparative performance and correctness evidence.
