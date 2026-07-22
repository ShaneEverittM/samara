# Phase 2 Audit: Executable Acceptance Contract

- Status: Accepted by Shane
- Date: July 22, 2026
- Governing phase: `docs/goal-mode-checklist.md`, Phase 2
- Baseline: commit `8408509`

## Outcome

Phase 2 is complete and accepted. The root
`samara` crate now exposes the compiler-checked candidate API, the three
reference Components compile against it, V1-V11 map to named acceptance
scenarios, and the Component-kernel slice is proposed for freeze.

No runtime behavior was implemented. Placeholder live and controlled methods
remain honest errors, and scenarios that require them remain staged.

## Accepted Decisions

This audit freezes these decisions for Phase 3:

1. `Component::update(&self, &mut Model, Message) -> Command<Message>` is the v0
   Rust transition spelling. In-place Model mutation is exclusively owned and
   observationally pure.
2. A transition returns one composable Command. `Command::batch` represents
   zero or more grouped intents and promises no completion order.
3. The base `Component` contract is `Send` but not `Sync`. Per-Component
   serialization does not require concurrent access to configuration, so
   requiring `Sync` would constrain scheduler placement prematurely.
4. `ComponentRef<C>` is an inert logical address; `ComponentHandle<C>` is the
   separate live ingress capability.
5. The Actor PoC is not a compatibility target. It may supply private
   implementation ideas only when new acceptance tests prove conformance.
6. Later Command, Subscription, interaction, controlled-runtime, and
   live-runtime surfaces remain compile-checked candidates until their own
   phase tranche is activated and audited.

## Cutover Evidence

- Removed the old active Actor runtime, Actor example, and PoC fixtures.
- Promoted the candidate façade to `src/lib.rs`.
- Promoted `minimal`, `api_pressure`, and `framed_socket` to root examples.
- Removed the nested `design/api-sketch` crate to prevent API drift.
- Preserved the root manifest and lockfile; the cutover did not upgrade Tokio
  or unrelated dependencies.
- Named the transport-neutral logical stream `StreamDescriptor<T>` while
  retaining adapter-specific `bind_mpsc` live assembly and transport-neutral
  controlled stream methods.
- Separated `StreamDescriptor` binding identity from Component-local
  `SubscriptionId` in the onboarding examples.
- Added `#[must_use]` to `ReplyTo` with a compile-fail lint test.
- Added `tests/phase2_contract.rs` for active V1, V3, and V4 evidence and the
  non-`Sync` Component-kernel bound.

## Validation

| Gate | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Pass |
| `cargo test --all-targets` | Pass: 16 passed, 1 intentionally staged/ignored |
| `cargo test --doc` | Pass: 1 runnable doctest and 3 compile-fail contracts |
| `cargo clippy --all-targets -- -D warnings` | Pass |
| `RUSTDOCFLAGS='-D warnings' cargo doc --no-deps` | Pass |
| `git diff --check` | Pass after final formatting check |
| Active-code Actor vocabulary scan | No matches in `src`, `tests`, or `examples` |

The ignored test is
`minimal::tests::controlled_stream_updates_the_same_program`. It is explicitly
staged for controlled execution; it does not hide a Phase 2 failure.

## Invariant Impact

- Model remains the only mutable application state.
- Message remains the only transition input.
- The transition surface exposes no runtime, clock, I/O, lock, or spawn
  capability.
- World-facing finite work requires a typed EffectDescriptor.
- Component references do not expose state or live delivery.
- No API or acceptance test mandates a mailbox, task-per-Component, central
  loop, or global live ordering.
- Controlled behavior remains a separate runtime decision and cannot silently
  invoke live Drivers.

No ordering, causality, isolation, or determinism semantics changed, so Phase 2
does not require a new ADR.

## Findings Deliberately Left Open

The acceptance audit found these real decision gates:

- V4 cannot settle retained-Source mapper replacement or stale events after
  Source replacement until Phase 4 defines that lifecycle.
- V6/V10 need a richer semantic trace and an observer enabled/disabled seam;
  the current illustrative TraceEvent cannot prove them.
- Program assembly needs a fallible graph-validation story before its API slice
  freezes.
- Complete L1 Command inspection still needs sends, timers, batch structure,
  and stored mapper behavior before Phase 4 can claim uniform coverage.
- Request lifecycle, notification delivery failure, Driver termination,
  profile bindings, backpressure, runtime errors, and shutdown remain assigned
  to later phase gates in `docs/api-contract.md`.

These are not Phase 2 test failures because Phase 2 neither implements nor
freezes those surfaces. Their scenario ownership is explicit in
`docs/testing/v0-acceptance-matrix.md`.

## Failure Modes and Recovery

- **Accidental old-PoC reuse:** prevent by keeping Actor-era fixtures out of the
  active build and requiring new acceptance tests around any reused private
  technique.
- **Placeholder behavior mistaken for conformance:** prevent by leaving
  runtime-dependent scenarios staged and documenting that façade errors are not
  evidence.
- **Future implementation pressure changes the contract silently:** the
  Goal-mode template requires the phase to stop and present evidence.
- **Over-freezing unresolved policy:** only the Component-kernel slice is
  proposed for freeze; later slices remain explicit candidates.

Rollback is source-only: revert this cutover or recover the old PoC from commit
`8408509`. The removed nested `target` directory contained 248 MiB of generated
build artifacts and can be regenerated; no user data or external state changed.

## Manual Spot Check

Review these in order:

1. `docs/api-contract.md` — especially “Frozen for Phase 3” and “Deliberately
   Unfrozen Surfaces.”
2. `docs/testing/v0-acceptance-matrix.md` — confirm scenario ownership and the
   decision gates match the intended vision.
3. `examples/minimal.rs` — verify the onboarding path still feels shallow.
4. `examples/api_pressure.rs` — verify handwritten Protocol and request flow
   remain direct.
5. `examples/framed_socket.rs` — verify the demanding end state still reads as
   the intended application.

Acceptance marks the final Phase 2 checkbox and authorizes Phase 3. Any later
disagreement must revise the frozen contract explicitly rather than changing it
inside an implementation goal.
