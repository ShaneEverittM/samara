# Runtime Topology Options (Decision Recorded)

## Decision Status
- Runtime topology for v0 is decided in `docs/adr/0001-runtime-topology.md`.
- **Selected topology: Option A (Single App Mailbox)**.
- **Selected ordering guarantee: G1 (Global Total Order)**.
- Options B and C remain documented as future alternatives that require a superseding ADR.

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

## Comparison Matrix (Initial)
| Criterion | Option A: Single Mailbox | Option B: Hybrid | Option C: Actors |
|---|---|---|---|
| Determinism | High | Medium-High | Medium |
| Complexity | Low | Medium | High |
| Testability | High | Medium | Medium-Low |
| Throughput | Medium | Medium-High | High |
| Fault Isolation | Medium | High | High |

## Recommendation Framework
1. Define two representative workloads (normal and high contention).
2. Validate correctness invariants against Option A baseline semantics.
3. Measure throughput, tail latency, and shutdown behavior.
4. Compare results against migration trigger thresholds.
5. If thresholds are exceeded, evaluate B/C and propose a superseding ADR.

## Migration Trigger Checklist (A -> B/C)
- Queue depth shows sustained growth under target load.
- P95/P99 loop latency exceeds service-level targets.
- Single-loop CPU saturation blocks required throughput growth.
- Failure isolation requirements exceed single-mailbox containment.

## Required Evidence for Topology Reconsideration
- Determinism and command emission tests.
- Runtime loop integration tests.
- Cancellation and graceful shutdown tests.
- Failure-path tests mapping runtime faults to `Msg`.
- Backpressure/load test results and interpretation.
- Comparative A vs candidate (B or C) benchmark and operational complexity analysis.
