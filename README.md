# Samara

Samara is a framework for building asynchronous applications in Rust.

> Samara enables application programming on Tokio that is as side-effect-free
> as practical: state transitions are pure, effect descriptions are explicit
> values, and asynchronous execution is confined to controlled boundaries.

## Status

Samara's Phase 2 executable contract, Phase 3 Component kernel, Phase 4
declarative-work kernel, and Phase 5 controlled runtime are accepted. The
bounded Phase 6 live Tokio runtime is implemented and ready for its manual
audit. Controlled programs retain deterministic causal traces and logical time;
live programs now interpret the same typed Commands and Sources through
structured Drivers, one-shot Tokio `mpsc` bindings, and a narrow TCP byte
Source under ADR-0004. Phase 6 is not closed until its audit is accepted.

The earlier Actor proof of concept remains available in Git history at commit
`8408509`, but it is not a compatibility target for the new runtime.

## Design Contracts

- [Vision](docs/vision.md)
- [v0 milestone API contract](docs/api-contract.md)
- [Glossary](docs/glossary.md)
- [API guidance](docs/api-guidance.md)
- [Runtime topology and ordering ADR](docs/adr/0002-runtime-topology-and-ordering.md)
- [Controlled execution semantics ADR](docs/adr/0003-controlled-execution-semantics.md)
- [Initial live runtime semantics ADR](docs/adr/0004-initial-live-runtime-semantics.md)
- [Architecture test strategy](docs/testing/architecture-test-strategy.md)
- [V1-V11 acceptance matrix](docs/testing/v0-acceptance-matrix.md)
- [Goal-mode implementation checklist](docs/goal-mode-checklist.md)
- [Phase 2 audit packet](docs/audits/phase-2-executable-acceptance-contract.md)
- [Phase 3 audit packet](docs/audits/phase-3-component-kernel.md)
- [Phase 4 audit packet](docs/audits/phase-4-declarative-work-kernel.md)
- [Phase 5 audit packet](docs/audits/phase-5-controlled-execution.md)
- [Phase 6 audit packet](docs/audits/phase-6-live-tokio-runtime.md)

## Standalone Examples

Each example is its own workspace package, so its application dependencies and
Samara boundary are visible without sharing the library crate's development
setup. The root validation commands still cover every example package.

- [`examples/minimal`](examples/minimal/src/main.rs) — the shallow Tokio
  `mpsc -> Component Message` onboarding path.
- [`examples/api_pressure`](examples/api_pressure/src/main.rs) — Ports, correlated
  requests, effects, subscriptions, and logical time.
- [`examples/framed_socket`](examples/framed_socket/src/main.rs) — source layering,
  TCP Driver shape, provider-neutral Protocols, failures, and structured
  shutdown.
- [`examples/time`](examples/time/src/main.rs) — a live HTTP time service and
  stdin-driven CLI using first-party HTTP, stdio, and terminal-input facilities.

## Validate the Contract

```sh
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo test --workspace --doc
cargo clippy --workspace --all-targets --all-features -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --all-features
```
