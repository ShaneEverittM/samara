# Samara

Samara is a framework for building asynchronous applications in Rust.

> Samara enables application programming on Tokio that is as side-effect-free
> as practical: state transitions are pure, effect descriptions are explicit
> values, and asynchronous execution is confined to controlled boundaries.

## Status

Samara's Phase 2 executable contract is accepted. The root crate exposes the
frozen Component-kernel API and compile-checked candidate surfaces for later
phases, but the live and controlled runtimes are not implemented yet. Direct
Component and API-contract tests run now; end-to-end runtime scenarios remain
visibly staged for their owning implementation phases.

The earlier Actor proof of concept remains available in Git history at commit
`8408509`, but it is not a compatibility target for the new runtime.

## Design Contracts

- [Vision](docs/vision.md)
- [v0 milestone API contract](docs/api-contract.md)
- [Glossary](docs/glossary.md)
- [API guidance](docs/api-guidance.md)
- [Runtime topology and ordering ADR](docs/adr/0002-runtime-topology-and-ordering.md)
- [Architecture test strategy](docs/testing/architecture-test-strategy.md)
- [V1-V11 acceptance matrix](docs/testing/v0-acceptance-matrix.md)
- [Goal-mode implementation checklist](docs/goal-mode-checklist.md)
- [Phase 2 audit packet](docs/audits/phase-2-executable-acceptance-contract.md)

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
