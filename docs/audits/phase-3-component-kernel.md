# Phase 3 Audit: Component Kernel

- Status: Accepted by Shane
- Date: July 22, 2026
- Governing phase: `docs/goal-mode-checklist.md`, Phase 3
- Baseline: commit `ba0d90f`

## Outcome

The bounded Phase 3 Component kernel is implemented. An assembled Program now
owns each Component's immutable configuration, exclusively owned Model, and
optional startup Command after one initialization. Exclusive mutable access to one
private Component kernel makes overlapping transitions impossible; execution
profiles may add a local concurrent-entry guard without introducing a global
execution order or a public mailbox, task, queue, or lock contract.

No Command is interpreted, no Subscription is reconciled, and neither runtime
profile executes application work in this phase. The compile-checked Phase 4+
façade remains an honest placeholder.

## Implemented Slice

`src/component_kernel.rs` contains the private runtime-owned mechanism for one
assembled Component:

- `ComponentKernel<C>` retains the explicit `ComponentId`, configured `C`,
  `C::Model`, and initial `Command<C::Message>`.
- A built Program has invoked `Component::init` exactly once and owns both the
  configuration and resulting Model. Whether initialization happens at the
  `component` call or later in `build` is not made observable by this phase.
- `transition` accepts only `C::Message` and invokes the frozen
  `update(&self, &mut Model, Message) -> Command<Message>` spelling.
- The Program-owned kernel exposes transition through `&mut self` and Model
  inspection through `&self`. This preserves synchronous controlled execution
  and makes concurrent mutation structurally impossible.
- A separate private `SerializedComponent<C>` demonstrates one replaceable
  local guard for an execution profile that accepts concurrent submissions. It
  does not become Program storage or constrain the controlled profile.
- The base `Component` contract remains `Send`, not `Sync`; the concurrency test
  executes a deliberately non-`Sync` configuration through the kernel.
- Startup work is retained and can be claimed once by a later declarative-work
  interpreter; Phase 3 does not execute it.

`ProgramBuilder::component` now stores heterogeneous Component kernels rather
than dropping the configuration. `Program`, `LiveRuntime`, `RuntimeTask`, and
`ControlledRuntime` privately carry that ownership through their existing
assembly shapes. Their later behavior is still unimplemented.

Type erasure is used only so the Program can own heterogeneous kernels. It does
not erase the typed `ComponentKernel<C>::transition(C::Message)` boundary or
expose an untyped public delivery path.

## Acceptance Evidence

| Requirement | Phase 3 evidence | Remaining tranche |
| --- | --- | --- |
| V1 repeatable transition | `v1_same_input_produces_equivalent_model_and_command_intent` repeats equivalent direct transitions; `v1_update_has_no_runtime_capability` compile-checks the exact frozen method type; a compile-fail doctest rejects another message vocabulary. | Stored EffectOutcome mapper equivalence remains Phase 5. |
| V2 isolated ownership | `registration_transfers_configuration_and_model_ownership_to_program` proves a built Program initialized once and owns both values until Program ownership ends without freezing registration-versus-build timing. `v2_same_component_transitions_never_overlap` pauses one transition at a test-only critical-section boundary, proves a second concurrent submission cannot enter, then lets both commit without wall-time or worker-count assumptions. The `ComponentHandle` compile-fail doctest rejects Model mutation. `v2_protocol_message_remains_distinct_from_provider_message` preserves the conversion boundary. | Command-driven cross-Component delivery remains staged until declarative work can be interpreted; live ingress repeats serialization in Phase 6. |
| V8 local serialization | `v8_component_transitions_do_not_overlap` exercises concurrent entry without naming or observing a mailbox, task, queue, registration order, or global scheduler sequence. | Controlled causality is Phase 5; independent live ordering is Phase 6; the partial-order audit is Phase 7. |

The Phase 2 V3 and V4 façade evidence remains green but is not claimed as Phase
3 implementation. It continues to prove typed descriptor intent and pure
Subscription projection only.

## Topology and Ordering Audit

The Program-owned kernel requires exclusive mutable access for a transition.
The optional concurrent-entry guard is private and scoped to one Component
instance. Its concrete lock implementation is replaceable runtime machinery,
not an application-visible guarantee. In particular:

- No shared program-wide guard was added.
- Controlled execution can own and borrow the synchronous kernel directly; it
  is not forced through Tokio locking or an embedded asynchronous executor.
- No registration ordinal is stored or exposed as Component identity.
- Tests assert only non-overlap and final transition count; they do not assert
  acquisition order or FIFO delivery.
- No relative order is assigned to independent Components or live events.
- The kernel exposes no public scheduler, queue, task, or lock handle.

Any fairness behavior of the current private lock is incidental and must not be
promoted into delivery semantics without a later contract decision.

## Reference Component and Public API Audit

The three reference Components continue to compile unchanged against the
frozen public Component API:

- `examples/minimal/src/main.rs` keeps the shallow boundary: immutable
  `StreamDescriptor` configuration, one small Model, typed Messages, and a
  direct synchronous transition.
- `examples/api_pressure/src/main.rs` demonstrates that startup timer intent
  remains an inert Command rather than being executed by Component initialization.
  Request, effect, and Subscription interpretation remain staged.
- `examples/framed_socket/src/main.rs` continues to separate immutable Port wiring from
  mutable telemetry state and operational Driver/Layer resources.

No public type or method was added for the kernel. `ComponentRef` remains an
inert logical address, and `ComponentHandle` remains a distinct live boundary
capability with no Model access. The frozen `Component`, `Init`, `Command`, and
`Subscriptions` signatures did not change.

The placeholder `Program`, `LiveRuntime`, `RuntimeTask`, and
`ControlledRuntime` unit representations became opaque structs with private
ownership fields. Their documented construction and method signatures are
unchanged, and their representation belongs to the deliberately unfrozen
runtime candidate slice. This prevents Component state from being dropped while
it passes through existing assembly shapes; it does not freeze later runtime
policy.

## Invariant Impact

- Model is now retained as runtime-owned state instead of being discarded by
  the façade.
- Message is the only typed input accepted by the kernel transition path.
- Configuration and Model require an exclusive kernel borrow during a
  transition; profiles that admit concurrent entry must provide an equivalent
  local serialization guard.
- `update` remains synchronous and receives no runtime, clock, I/O, locking, or
  spawning capability.
- Commands remain inert returned values; startup and transition Commands are
  not executed in Phase 3.
- No observable ordering, causality, controlled-determinism, or topology
  contract changed, so no new ADR is required.

Practical purity remains a shared conformance obligation. Rust and the kernel
cannot prove that arbitrary user code avoids globals, direct I/O, interior
mutability, or nondeterminism; the API keeps those capabilities out of the
normal transition path and the direct V1 test supplies representative evidence.

## Failure Modes and Recovery

- **Component panic:** panic containment and the status of a partially mutated
  Model are not specified by the frozen Phase 3 contract. Expected failures
  must remain typed Messages rather than panic control flow. A later runtime
  phase must make supervision behavior explicit before claiming fault
  isolation.
- **Duplicate logical identity:** graph validation and the fallible
  `ProgramBuilder::build` boundary remain an explicit Phase 4+ decision gate.
  Phase 3 does not choose a duplicate-ID policy.
- **Cancelled transition waiter:** no public ingress or delivery acceptance
  contract exists yet. Cancellation, backpressure, and queueing semantics remain
  assigned to the runtime phases.
- **Uninterpreted work:** initial and returned Commands are retained or returned
  but deliberately not dispatched. Treating that as executed work would be a
  test error until Phase 4 activates Command interpretation.
- **Non-conforming Component:** hidden mutable configuration or ambient effects
  can still violate observational purity. The runtime cannot repair such user
  code; documentation and later conformance practice must diagnose it.

Rollback is source-only: revert the private kernel, Program ownership fields,
Phase 3 tests, and this audit packet. No external state, persistent data, or
live world resource is created by this slice.

## Validation

An independent read-only diff audit returned **GO** after two findings were
fixed: Program storage was changed from an asynchronous mutex-backed kernel to
a synchronous exclusively borrowed kernel, and the concurrency evidence was
made deterministic without sleeps or blocking inside `update`.

| Gate | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Pass |
| `cargo test --all-targets` | Pass: 24 passed, 1 intentionally staged/ignored |
| `cargo test --doc` | Pass: 1 runnable doctest and 6 compile-fail contracts |
| `cargo clippy --all-targets -- -D warnings` | Pass |
| `RUSTDOCFLAGS='-D warnings' cargo doc --no-deps` | Pass |
| `git diff --check` | Pass |
| Topology/vocabulary scope scan | Pass: no Phase 4 interpretation in the kernel tests or implementation; Actor appears only in README's historical note |

## Manual Spot Check

Review these in order:

1. `src/component_kernel.rs` — confirm exclusive kernel ownership plus the
   optional private local guard establish only per-Component serialization and
   that startup work remains inert.
2. `tests/phase3_component_kernel.rs` — confirm identity and ownership evidence
   does not depend on registration order or runtime topology.
3. `src/lib.rs` around `ProgramBuilder::component` — confirm ownership transfers
   into the Program without changing the frozen public signature.
4. The V1, V2, and V8 rows in `docs/testing/v0-acceptance-matrix.md` — confirm
   later Command delivery, causality, and live-runtime tranches remain staged.
5. The three reference Components — confirm their application-facing shape
   still directly expresses immutable wiring, owned state, typed input, and
   inert work declarations.

Shane accepted this audit on July 22, 2026. Phase 3 is closed and Phase 4 is
authorized.
