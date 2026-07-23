# Samara

Samara is a framework for building asynchronous applications in Rust.

> Samara enables application programming on Tokio that is as side-effect-free
> as practical: state transitions are pure, effect descriptions are explicit
> values, and asynchronous execution is confined to controlled boundaries.

## Status

Samara's Phase 2 executable contract, Phase 3 Component kernel, and Phase 4
declarative-work kernel are accepted. The Phase 5 implementation is ready for
manual audit: controlled programs now interpret typed Commands, maintain and
compose Sources, advance logical time, route successful Requests, account for
semantic obligations, and collect deterministic causal traces under ADR-0003.
Controlled execution never invokes a live Driver. Live Tokio Drivers and
structured live shutdown remain the Phase 6 boundary.

The earlier Actor proof of concept remains available in Git history at commit
`8408509`, but it is not a compatibility target for the new runtime.

## Design Contracts

- [Vision](docs/vision.md)
- [v0 milestone API contract](docs/api-contract.md)
- [Glossary](docs/glossary.md)
- [API guidance](docs/api-guidance.md)
- [Runtime topology and ordering ADR](docs/adr/0002-runtime-topology-and-ordering.md)
- [Controlled execution semantics ADR](docs/adr/0003-controlled-execution-semantics.md)
- [Architecture test strategy](docs/testing/architecture-test-strategy.md)
- [V1-V11 acceptance matrix](docs/testing/v0-acceptance-matrix.md)
- [Goal-mode implementation checklist](docs/goal-mode-checklist.md)
- [Phase 2 audit packet](docs/audits/phase-2-executable-acceptance-contract.md)
- [Phase 3 audit packet](docs/audits/phase-3-component-kernel.md)
- [Phase 4 audit packet](docs/audits/phase-4-declarative-work-kernel.md)
- [Phase 5 audit packet](docs/audits/phase-5-controlled-execution.md)

## Reference Components

- [`examples/minimal.rs`](examples/minimal.rs) — the shallow Tokio
  `mpsc -> Component Message` onboarding path.
- [`examples/api_pressure.rs`](examples/api_pressure.rs) — Ports, correlated
  requests, effects, subscriptions, and logical time.
- [`examples/framed_socket.rs`](examples/framed_socket.rs) — source layering,
  TCP Driver shape, provider-neutral Protocols, failures, and structured
  shutdown.

## Validate the Contract

```sh
cargo fmt --all -- --check
cargo test --all-targets
cargo test --doc
cargo clippy --all-targets -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --no-deps
```
