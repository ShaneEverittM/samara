# Runtime Topology Options (Historical Analysis)

## Decision Status
- ADR-0001 originally selected Option A and G1 for v0.
- ADR-0002 supersedes that selection and does not mandate a runtime topology.
- Options A, B, and C remain valid implementation techniques when they preserve
  per-Component serialization, explicit causality, and controlled determinism.
- No implementation may expose incidental global serialization as a portable
  application guarantee.

## Candidate Topologies

### Option A: Single App Mailbox
- One central queue and one update loop for all messages.
- Effect handlers execute concurrently on Tokio and post results back to the single mailbox.

### Option B: Hybrid Root + Child Loops
- One root loop coordinates global concerns.
- Optional child loops handle isolated subsystems and communicate through typed messages.

### Option C: Per-Domain Actors
- Multiple actor-like loops own separate domain models.
- Coordination happens through inter-actor message channels.

## Evaluation Criteria
- Determinism.
- Complexity (design and operational burden).
- Testability.
- Throughput under load.
- Fault isolation and recovery boundaries.
- Simulation compatibility (deterministic virtual-time control and faster-than-real-time viability).

## Ordering Guarantee Candidates

### Guarantee 1: Global Total Order
- Every message is processed in a single global sequence.
- Best alignment: Option A.
- Tradeoff: simplest reasoning, potential throughput limits.

### Guarantee 2: Per-Source FIFO with Causal Handoff
- FIFO per source/channel, with explicit causal boundaries across sources.
- Best alignment: Option B.
- Tradeoff: moderate complexity with targeted scalability.

### Guarantee 3: Partitioned/Eventual Processing
- Ordering guaranteed only within partitions/actors.
- Best alignment: Option C.
- Tradeoff: highest scalability and isolation, weakest global ordering story.

## Comparison Matrix (Historical, Qualitative)
| Criterion | Option A: Single Mailbox | Option B: Hybrid | Option C: Actors |
|---|---|---|---|
| Live scheduling simplicity | High | Medium-High | Medium |
| Complexity | Low | Medium | High |
| Testability | High | Medium | Medium-Low |
| Throughput | Medium | Medium-High | High |
| Fault Isolation | Medium | High | High |

Controlled determinism is required of every conforming topology and is not a
relative advantage of Option A.

## Recommendation Framework
1. Define two representative workloads (normal and high contention).
2. Validate the ADR-0002 observable semantics for every candidate.
3. Measure throughput, tail latency, and shutdown behavior.
4. Compare candidates without treating incidental global order as correctness.
5. Select or change topology using implementation evidence; require an ADR only
   when observable semantics would change.

## Implementation Reconsideration Checklist
- Queue depth shows sustained growth under target load.
- P95/P99 loop latency exceeds service-level targets.
- Scheduler CPU saturation blocks required throughput growth.
- Failure isolation requirements exceed the current implementation's boundaries.

## Required Evidence for a Topology Selection or Change
- Per-Component serialization and command emission tests.
- Causal message-flow integration tests.
- Repeated controlled-execution determinism tests.
- Cancellation and graceful shutdown tests.
- Failure-path tests mapping runtime faults to `Msg`.
- Backpressure/load test results and interpretation.
- Comparative current-versus-candidate benchmark and operational complexity analysis when replacing an implementation.
