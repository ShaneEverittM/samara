# Samara Goal-Mode Checklist

- Status: Active
- Purpose: Track bounded implementation phases and their human audit gates.

## Working Rule

Each implementation phase is a separate Goal-mode run with executable completion
criteria. Do not advance until Shane accepts its audit. If implementation
reveals a contract conflict, stop rather than silently changing a governing
document.

Governing documents:

- [Samara Vision](vision.md)
- [ADR-0002: Runtime Topology and Ordering Semantics](adr/0002-runtime-topology-and-ordering.md)
- [Architecture Test Strategy](testing/architecture-test-strategy.md)

## Phases

### 0. Direction and Runtime Semantics

- [x] Establish the vision and executable requirements V1-V11.
- [x] Remove the single-mailbox and global-order mandate through ADR-0002.
- [x] Align existing architecture and testing guidance.

### 1. Consumer-Driven API Contract

- [ ] Sketch the precise public API without prescribing internal topology.
- [ ] Exercise it with both a smallest-useful `mpsc -> Msg` Component and a
  demanding framed-socket Component.
- [ ] Audit readability, ergonomics, explicitness, and whether each Component
  pays for its ceremony.

### 2. Executable Acceptance Contract

- [ ] Map V1-V11 to scenarios and tests, compile the reference Components against
  stubs, and freeze the first-milestone API.
- [ ] Audit for missing semantics, topology leakage, and untestable promises.

### 3. Component Kernel

- [ ] Implement Component identity, state ownership, typed messages, and pure
  serialized transitions; pass their acceptance tests.
- [ ] Audit the reference Components and public API before continuing.

### 4. Controlled Execution

- [ ] Implement controlled effects, subscriptions, scheduling, and logical time;
  demonstrate repeatable program-wide traces and final state.
- [ ] Audit causality, equal-time behavior, pending-work accounting, and failure
  diagnostics.

### 5. Effects, Subscriptions, and Adapters

- [ ] Implement interceptable commands, subscription reconciliation, the
  first-party `mpsc` bridge, and the framed-socket adapter.
- [ ] Audit lifecycle ownership, failure-as-message behavior, and controlled/live
  parity.

### 6. Live Tokio Runtime

- [ ] Implement live execution and structured shutdown; characterize load,
  backpressure, cancellation, and faults.
- [ ] Run both reference Components unchanged in live and controlled profiles.
- [ ] Audit observable parity and any incidental topology assumptions.

### 7. Conformance and Release Readiness

- [ ] Pass the topology-independent conformance suite and all V1-V11 scenarios.
- [ ] Complete onboarding docs and record known gaps, failure modes, and deferrals.
- [ ] Perform a final vision and developer-experience audit.

## Goal Template

```text
Implement Samara phase <N> only. Conform to docs/vision.md,
docs/adr/0002-runtime-topology-and-ordering.md, the frozen API contract, and the
phase acceptance tests. Do not change those contracts. If they conflict or are
not implementable as written, stop and present evidence. Complete only when the
phase checks pass and an audit summary is ready for review.
```
