# Architecture Test Strategy (v0)

## Status
- Purpose: the testing rules that Samara's conformance tests follow today.
  This document is normative for PR review of test changes.
- Evidence: [`v0-acceptance-matrix.md`](v0-acceptance-matrix.md) is the live
  traceability record from scenario to executable test or compile-fail
  doctest. This document does not repeat its scenario lists.
- Date: trimmed November 8, 2026 to the rules current tests demonstrably
  follow. The July 25, 2026 planning text and the phase-by-phase layer
  catalogue are gone. Phase 6 was accepted on September 22, 2026.

## Where Evidence Lives
- `tests/*.rs` holds the integration suites, one file per contract or ADR
  tranche.
- Rustdoc `compile_fail` examples in [`src/lib.rs`](../../src/lib.rs) hold the
  compile contracts, under a "Compile-time checks" block on the item they
  constrain.
- Private kernel tests in
  [`src/component_kernel.rs`](../../src/component_kernel.rs) cover concurrent
  serialization below the public API.
- The `examples/` packages test their Components directly and run them through
  both profiles.

## Rules

### Direct Component tests first
- Verify transitions are deterministic and side-effect free for fixed immutable
  Component configuration. Equivalent configuration + Model + Message input
  must yield equivalent next Model and Command intent.
- Validate Message-to-Command mapping. Inspect typed descriptors without
  executing live work and exercise pure message mappers with equivalent
  outcomes or events.
- Compare domain descriptor payloads in direct typed-intent tests rather than
  assuming generic trace capture. Controlled equivalence tests compare
  structural trace and final state; direct Component tests separately compare
  same-typed descriptor payloads.
- Keep `update` free of runtime capability. A test may pin the frozen signature
  `fn(&Self, &mut Model, Message) -> Command<Message>` to prove no runtime
  handle is reachable.

Evidence: `tests/phase2_contract.rs`, `tests/phase3_component_kernel.rs`,
`tests/phase4_declarative_work.rs`.

### Controlled execution first
- Every boundary gets controlled evidence before, or alongside, live evidence.
  Controlled execution supplies deterministic effect outcomes and source
  events without silently invoking live Drivers.
- Confirm controlled profile build rejects every declared terminal requirement
  without controlled behavior and never falls back to a live Driver.
- Confirm determinism across repeated runs with identical inputs. Identical
  controlled runs must have equal structural trace and final state; equal
  final state with a different structural trace is not equivalent.
- Runtime drive-loop tests prefer `run_until(...)` /
  `run_until_predicate(...)` / `run_until_idle()` over hard-coded sleep
  durations. No test in `tests/` sleeps on the wall clock. Live timeout tests
  use paused Tokio time; controlled tests advance logical time.

Evidence: `tests/phase5_controlled_execution.rs`, `tests/request_timeouts.rs`.

### Trace-based assertions
- Controlled trace tests must verify the common record envelope, one immediate
  causal parent for every non-root, roots with no parent, logical time, Source
  stale-drop records, and read-afterward noninterference without requiring a
  streaming callback.
- Trace assertions describe only the raw terminal outcome of a boundary. A
  pipeline or Layer above it is asserted through the resulting Component
  Message, not through extra trace records.
- Equal-time work follows logical deadline plus deterministic insertion ticket
  ([ADR-0003](../adr/0003-controlled-execution-semantics.md)). Tests may rely
  on it in controlled execution as reproducibility machinery, not as a
  cross-profile domain guarantee.

Evidence: `tests/phase5_controlled_execution.rs`,
`tests/http_response_pipeline.rs`, `tests/http_redirects.rs`.

### Topology-neutral assertions
- Confirm required causal edges without assuming an order between independent
  live events.
- Conformance compares a causal partial order and deliberately runs
  independent events in more than one valid order. It never blesses one
  observed interleaving as the program contract.
- The matrix's
  [Topology-Neutral Assertion Rules](v0-acceptance-matrix.md#topology-neutral-assertion-rules)
  list what may and must not be asserted.
  [ADR-0002](../adr/0002-runtime-topology-and-ordering.md) owns the semantics.

Evidence: `tests/phase6_live_runtime.rs`
(`v8_independent_live_events_accept_either_order`,
`v8_conformance_compares_partial_order_not_scheduler_sequence`).

### Per-Component serialization
- Confirm that transitions for one Component never overlap.
- Private kernel tests hold the first transition open inside its critical
  section and prove the second cannot enter until the first is released,
  independent of worker count, wall time, or queue shape. Live tests admit a
  burst through a handle and assert the full set arrives with nothing lost.

Evidence: `src/component_kernel.rs`
(`v2_same_component_transitions_never_overlap`),
`tests/phase6_live_runtime.rs`
(`phase6_live_ingress_serializes_component_transitions`).

### Compile-fail doctests for shapes that must not be writable
- Compile-contract tests must prove raw EffectDescriptors cannot construct
  Effect Commands and raw SourceDescriptors cannot construct Subscriptions.
- Validate the public descriptor and Command boundary cannot substitute an
  opaque async closure for an identifiable EffectDescriptor or a synchronous
  message mapper.
- Public API lint checks must verify that discarding a `ReplyTo`-valued
  expression or a `Command` triggers its `#[must_use]` diagnostic under
  `#![deny(unused_must_use)]`, without claiming that the lint proves eventual
  consumption.
- Compile-contract checks must keep `PortHandle` and `ComponentHandle`
  distinct from inert `Port` and `ComponentRef`, expose no Model access, and
  prove a handle cannot substitute in Command or ControlledRuntime APIs.
  Because Rust cannot reject arbitrary fields in a `Send` Component
  configuration, keeping live handles out of Components also remains an
  explicit conformance rule.
- Each check lives in the rustdoc of the item it constrains, inside a
  `<details><summary>Compile-time checks</summary>` block, so `cargo test
  --doc` runs it and the matrix can cite it by item name.

Evidence: `src/lib.rs` (`Component`, `Command`, `ReplyTo`,
`EffectCapability`, `SourceCapability`, `ComponentHandle`, `PortHandle`,
`Assembly`).

### Integration tests assert across public boundaries
- Suites in `tests/` import `samara::prelude` and nothing private. They
  validate message ingestion, target delivery, update execution, command
  dispatch, and message re-entry through the same APIs an application uses.
- Each terminal EffectDescriptor and SourceDescriptor is covered through both
  a live Driver test double and controlled behavior. Layers must compose
  descriptors and outcomes/events identically in live and controlled
  execution without ambient I/O.
- Expected failures whose contracts define typed Error data enter through
  explicit Component Messages or typed boundary outcomes/events. Live
  mechanism faults that cannot truthfully construct typed application data,
  including Driver panic, must close admission, cancel/join owned work, and
  surface `RuntimeError` through the host boundary.
- Live tests against a network or process boundary run a local server or an
  injected stream, never ambient process input or a remote host.

Evidence: `tests/phase6_live_runtime.rs`
(`v5_live_and_controlled_boundaries_map_equivalent_observations`),
`tests/http_response_pipeline.rs`, `tests/external_port_ingress.rs`,
`tests/stdin_source.rs`.

### Work accounting and shutdown
- Confirm successful shutdown reports zero remaining, `pending_now`, and
  `pending_later` obligations without depending on exact completed/cancelled
  diagnostic counts.
- Controlled cancellation tests reduce both pending counts to zero. Scope
  cancellation manufactures no Request or effect outcome and invokes no mapper
  solely because the scope ended.
- The matrix's
  [Structured-Ownership Observation Points](v0-acceptance-matrix.md#structured-ownership-observation-points)
  define what each count includes and where it may be read.

Evidence: `tests/phase5_controlled_execution.rs` (`v9_*`),
`tests/phase6_live_runtime.rs` (`phase6_successful_shutdown_owns_zero_work`).

### Characterization, not thresholds
- For v0 unbounded internal delivery, prove a healthy running scope does not
  intentionally drop successfully accepted work in a bounded test.
- Record workload and result as characterization only. Do not turn capacity
  or performance values into stable conformance thresholds or imply support
  for memory exhaustion.

Evidence: `tests/phase6_live_runtime.rs`
(`phase6_accepted_internal_delivery_has_no_silent_drop`).

## Gates
- Architecture or contract changes must update the architecture docs, the
  relevant ADR, and the matrix row before the tests go green.
- Changes to observable ordering, causality, isolation, or
  controlled-determinism semantics require an ADR and matching evidence.
  Topology-coupled implementation changes without a semantic change require
  no ADR but must pass the existing partial-order and serialization suites
  unchanged.
- Required PR contents are listed in [`AGENTS.md`](../../AGENTS.md).

## Notes for v0
- Persistence/replay tests are explicitly excluded from v0 scope.
