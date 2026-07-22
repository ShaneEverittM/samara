# Samara Goal-Mode Checklist

- Status: Phase 2 complete; Phase 3 ready
- Purpose: Track bounded implementation phases and their human audit gates.

## Working Rule

Each implementation phase is a separate Goal-mode run with executable completion
criteria. Do not advance until Shane accepts its audit. If implementation
reveals a contract conflict, stop rather than silently changing a governing
document.

Acceptance scenarios may be specified before the runtime machinery needed to
execute them exists. Each implementation phase activates its relevant scenario
tranche before implementation begins; the complete suite becomes mandatory in
Phase 7. A staged scenario is a tracked requirement, not a passing test.

Governing documents:

- [Samara Vision](vision.md)
- [Samara Glossary](glossary.md)
- [API Guidance](api-guidance.md)
- [v0 Milestone API Contract](api-contract.md)
- [ADR-0002: Runtime Topology and Ordering Semantics](adr/0002-runtime-topology-and-ordering.md)
- [Architecture Test Strategy](testing/architecture-test-strategy.md)
- [v0 Acceptance Matrix](testing/v0-acceptance-matrix.md)

## Phases

### 0. Direction and Runtime Semantics

- [x] Establish the vision and executable requirements V1-V11.
- [x] Remove the single-mailbox and global-order mandate through ADR-0002.
- [x] Align existing architecture and testing guidance.

### 1. Consumer-Driven API Contract

- [x] Sketch the precise public API without prescribing internal topology.
- [x] Exercise it with both a smallest-useful `mpsc -> Component Message` Component and a
  demanding framed-socket Component.
- [x] Audit readability, ergonomics, explicitness, and whether each Component
  pays for its ceremony.
- [x] Shane accepted the Phase 1 API direction and authorized cutover.

### 2. Executable Acceptance Contract

- [x] Promote the compiler-checked API sketch into the root `samara` crate and
  retire the old Actor PoC from the active build.
- [x] Map V1-V11 to named, topology-neutral scenarios and implementation phases.
- [x] Compile the reference Components and executable Phase 2 tests against the
  root crate's unimplemented runtime façade.
- [x] Establish the candidate first-milestone API freeze and explicitly record
  policy-bearing surfaces that remain deferred.
- [x] Audit for missing semantics, topology leakage, and untestable promises.
- [x] Shane accepts the Phase 2 audit and freezes the Component-kernel contract.

Audit packet: [Phase 2 Executable Acceptance Contract](audits/phase-2-executable-acceptance-contract.md).

### 3. Component Kernel

- [ ] Implement Component identity, state ownership, typed messages, and pure
  serialized transitions; pass their acceptance tests.
- [ ] Audit the reference Components and public API before continuing.

### 4. Declarative Work Kernel

- [ ] Implement interceptable `Command` values carrying typed
  `EffectDescriptor` values and one-shot message mappers, plus `Subscription`
  reconciliation over identity, `SourceDescriptor`, and reusable message
  mappers.
- [ ] Implement the first compositional Layers, including the framed-socket
  Layer, without selecting live or controlled terminal behavior.
- [ ] Audit `EffectOutcome` and `SourceEvent` mapping into `Message`, Source
  retention and replacement, and the separation between Layers and Drivers.

### 5. Controlled Execution

- [ ] Implement controlled terminal-descriptor behavior, Source maintenance,
  scheduling, and logical time; demonstrate repeatable program-wide traces and
  final state with both reference Components.
- [ ] Audit causality, equal-time behavior, pending-work accounting, failure
  diagnostics, and proof that controlled execution never silently invokes a
  live Driver.

### 6. Live Tokio Runtime

- [ ] Implement live execution, terminal EffectDrivers and SourceDrivers,
  first-party `mpsc` and TCP Source Drivers, and structured shutdown;
  characterize load, backpressure, cancellation, and faults through
  runtime-scoped Sources and Drivers.
- [ ] Run both reference Components unchanged in live and controlled profiles.
- [ ] Audit observable parity and any incidental topology assumptions.

### 7. Conformance and Release Readiness

- [ ] Pass the topology-independent conformance suite and all V1-V11 scenarios.
- [ ] Complete onboarding docs and record known gaps, failure modes, and deferrals.
- [ ] Perform a final vision and developer-experience audit.

## Goal Template

```text
Implement Samara phase <N> only. Conform to docs/vision.md,
docs/glossary.md, docs/api-guidance.md,
docs/api-contract.md, docs/adr/0002-runtime-topology-and-ordering.md, and
docs/testing/v0-acceptance-matrix.md. Implement only the active contract slice
and phase acceptance tests. Do not change governing contracts. If they conflict
or are not implementable as written, stop and present evidence. Complete only
when the phase checks pass and an audit summary is ready for review.
```
