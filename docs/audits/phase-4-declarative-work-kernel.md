# Phase 4 Audit: Declarative Work Kernel

- Status: Accepted by Shane
- Date: July 22, 2026
- Governing phase: `docs/goal-mode-checklist.md`, Phase 4
- Baseline: commit `c629c4c`

## Outcome

The bounded Phase 4 declarative-work kernel is implemented. A Command can now
be flattened into inert declarations and an effect declaration can be
intercepted as an owned, typed `EffectInvocation<E, Message>`. This preserves a
non-`Clone` EffectDescriptor separately from its one-shot Message mapper without
executing world work.

Each Component kernel also owns descriptor-only Subscription bookkeeping. It
classifies current Model-derived desire as start, retain, replace, or cancel
under Component-local identity. The bookkeeping creates no Source and carries
newly declared mapper data forward without choosing the retained-Source mapper
policy that remains an explicit Phase 5 decision gate.

`FramedLayer` is the first implemented compositional Source Layer. It owns only
deterministic decoder state and transforms inner SourceEvents into outer typed
SourceEvents. It neither invokes a SourceDriver nor delivers a Component
Message.

No terminal live or controlled behavior, Source realization, message delivery,
scheduling, logical time, Driver work, or runtime policy is implemented in this
phase.

## Implemented Slice

### Interceptable finite work

- `Command::into_declarations` consumes nested batches into individual inert
  declarations. It omits `Command::none`; traversal order is inspectable but
  does not promise execution or completion order.
- `Command::effect_intents` exposes every descriptor of one concrete type while
  preserving equal-looking occurrences as distinct invocations.
- `Command::into_effect` consumes one top-level effect declaration into
  `EffectInvocation<E, Message>` without requiring `E: Clone`.
- `EffectInvocation::descriptor` exposes the explicit typed intent and
  `EffectInvocation::map_outcome` consumes the stored `FnOnce` mapper. A
  compile-fail doctest proves the mapper cannot be invoked twice.
- Runtime-internal `EffectInvocation::into_parts` can move the owned descriptor
  toward later terminal behavior while retaining the matching mapper under
  runtime ownership. Phase 4 does not send either part anywhere.
- `Command::map_effect_outcome` remains the shorter direct-test path for a
  top-level effect declaration.

### Reusable ongoing-work mapping and reconciliation

- `Subscription::map_source_event` applies the stored `Fn` mapper repeatedly
  for the matching concrete SourceDescriptor and returns mismatched events
  unchanged.
- Reconciliation stores cloned, type-erased descriptor snapshots only. It never
  stores a live resource or treats mapper object identity as reconciliation
  identity.
- One reconciler is structurally owned by each `ComponentKernel`, so equal
  `SubscriptionId` values in different Components remain independent.
- A new identity produces `Start`; equal identity and descriptor produce
  `Retain`; changed descriptor data produces `Replace`; removed desire produces
  `Cancel`.
- Reconciliation commits descriptor bookkeeping as one planning step. The
  returned Vec is an unordered lifecycle-change collection, not Source
  start/stop sequencing; Phase 5 must accept the plan atomically or discard the
  owning kernel on an unrecoverable setup failure.
- Duplicate desired identities within one Component are rejected before
  bookkeeping changes. This is a private kernel diagnostic, not a frozen
  runtime error taxonomy.
- The `Retain` result carries the newly declared Subscription as candidate data,
  but the reconciler stores no mapper and selects neither the prior nor the new
  mapper for event delivery.

### Profile-independent framing Layer

- `Framed::into_layer` creates one `FramedLayer` with fresh decoder state and no
  world-facing resource.
- `FramedLayer::inner_descriptor` exposes the next descriptor in the composed
  stack without claiming it is terminal.
- `FramedLayer::map_event` maps raw items through the same stateful decoder for
  every execution profile, including zero-frame and multi-frame inputs.
- A composed `Framed` descriptor can itself be the inner descriptor of another
  `Framed` Layer; the Phase 4 test cascades typed events through both Layers
  without a profile binding.
- Underlying failures become `FramedError::Source`, decoder failures become
  `FramedError::Decode`, and normal inner ending becomes normal outer ending.
- The Layer emits typed data only. Runtime handling of terminal events and
  Source lifecycle remains assigned to Phase 5.

## Acceptance Evidence

| Requirement | Phase 4 evidence | Remaining tranche |
| --- | --- | --- |
| V3 interceptable descriptor | `v3_effect_command_exposes_descriptor_and_maps_one_outcome` intercepts an owned non-`Clone` descriptor and consumes its mapper. `v3_all_effect_outcomes_remain_typed_message_input` covers typed failure and cancellation. `v3_equal_effect_descriptors_remain_distinct_command_occurrences` and the private batched-occurrence test preserve two equal-looking invocations and their distinct continuations. Existing compile-fail tests continue to reject non-descriptors and async mappers. | Phase 5 must route the owned descriptor through controlled terminal behavior, invoke the mapper exactly once under runtime correlation, re-enter the Component as a Message, and prove no live Driver ran. |
| V4 declarative lifecycle | Private reconciliation tests cover start, retain, replace, cancel, duplicate desire, Component-local identity, and projection from a committed Component Model. `v4_source_event_mapper_is_reusable_before_delivery` proves repeated mapping, while `v4_framed_source_events_map_into_component_messages` composes Layer output with the Subscription mapper without runtime delivery. | Phase 5 must create and maintain Sources, select the retained-mapper policy, reject stale replaced-Source events, and deliver controlled events/failures as Messages. Phase 6 repeats lifecycle evidence through live Drivers. |

The existing Phase 2 projection tests and Phase 3 Component-kernel tests remain
green. Phase 4 does not claim V3 controlled execution or V4 Source maintenance
merely because pure descriptors, Layers, and mappers can now be exercised
directly.

## Layer and Driver Separation Audit

The Phase 4 implementation contains no Tokio call, future, Driver handle,
terminal binding, wall clock, scheduler, queue, task, or Source realization in
its declarative-work machinery.

`FramedLayer` accepts and returns typed SourceEvent values. The existing
`SourceDriver<D>` trait remains the separate live terminal boundary and is not
called by the Layer or reconciliation code. Controlled behavior remains absent;
there is no fallback path, live or otherwise.

The decoder's partial buffer is runtime-scoped deterministic Layer state. It is
not Component Model state, SourceDescriptor data, or a transport Driver
resource. Framing selection and error interpretation remain explicit in the
composed descriptor and Component Message logic.

## Topology and Ordering Audit

- Reconciliation state is local to one Component kernel; there is no
  program-wide registry or guard.
- Batch flattening preserves declaration traversal only for inspection. No
  effect start or completion ordering is asserted.
- No Component transition, message route, Source event, task, queue, mailbox,
  or scheduler order is introduced.
- The Phase 3 per-Component serialization mechanism is unchanged apart from
  carrying its own declarative Subscription bookkeeping.
- No observable ordering, causality, isolation, or controlled-determinism
  contract changed, so no ADR is required.

## Reference Component and Public API Audit

- `examples/minimal.rs` keeps the shallow StreamDescriptor-to-Message shape.
  Its direct Component test remains runtime-free, while the controlled
  end-to-end test remains visibly staged.
- `examples/api_pressure.rs` now intercepts its actual `PersistCount`
  descriptor and drives the stored mapper with typed data before feeding the
  resulting Message through an ordinary direct transition. It invokes no
  persistence Driver.
- `examples/framed_socket.rs` now instantiates `FramedLayer` from the actual
  `TelemetryFeed` descriptor and maps split/coalesced chunks plus transport
  failure without selecting `TokioTcpBytes` or controlled behavior.
- The accepted Phase 3 `Component`, `Init`, `ComponentRef`, identity, ownership,
  and serialization signatures are unchanged. The Phase 4 public additions are
  `EffectInvocation`, `Command::effect_intents`, `Command::into_effect`,
  `Command::into_declarations`, `Command::map_effect_outcome`,
  `Subscription::map_source_event`, `Framed::into_layer`, and `FramedLayer`.
- Program graph, Request lifecycle, notification delivery failure, execution
  profile bindings, Source realization, and runtime surfaces remain
  provisional and unchanged.

## Invariant Impact

- EffectDescriptor values remain inert and separately identifiable after type
  erasure; they can be moved without cloning while their one-shot mapper stays
  attached to the same invocation.
- EffectOutcome and SourceEvent mapping remains synchronous application logic
  with no runtime capability. Purity and determinism remain author conformance
  obligations.
- Subscription equality compares only Component-local identity plus typed
  SourceDescriptor equality. Mapper object identity is excluded.
- Subscription projection is evaluated from the Component's committed Model;
  reconciliation itself does not mutate Model.
- Framed composition is identical before either profile reaches a terminal
  boundary and contains no ambient I/O.
- Commands, changes, and Layer outputs are still inert data. Phase 4 creates no
  external state and authorizes no asynchronous work.

## Failure Modes and Recovery

- **Wrong typed inspection:** `Command::into_effect` and
  `Subscription::map_source_event` return the original Command or SourceEvent
  unchanged when the requested descriptor type does not match. Checked type
  erasure couples each descriptor to its associated outcome or event type.
- **Duplicate desired identity:** reconciliation returns a private diagnostic
  before changing descriptor bookkeeping. Runtime error presentation is not
  frozen by this choice.
- **Unapplied reconciliation plan:** descriptor bookkeeping advances when a
  valid plan is produced. Phase 5 must treat accepting and scheduling that plan
  as one runtime step; it must not continue with the same kernel after silently
  dropping lifecycle changes.
- **Non-conforming mapper or decoder:** arbitrary user code may panic, read
  ambient state, or otherwise violate determinism. The API supplies no runtime
  capability, but panic containment and conformance diagnostics remain later
  runtime and testing work.
- **Retained mapper:** Phase 4 deliberately does not decide whether an equal
  descriptor adopts a newly declared mapper. No Source event delivery exists
  that could make an accidental choice observable.
- **Replaced-Source stale event:** Source generations and stale-event rejection
  require Source realization and remain Phase 5 work.
- **Layer terminal event:** a decoder or inner-source failure is represented as
  typed terminal data, but Phase 4 does not stop a running Source. Phase 5 must
  enforce that lifecycle.
- **Incomplete decoder state at normal end:** the current candidate `Decoder`
  contract has no finalization hook. `FramedLayer` therefore maps inner normal
  ending directly to outer normal ending. Shane accepted deferring the exact
  EOF contract so it can be designed together with the planned move to the
  `bytes` crate. This behavior is a documented limitation, not a settled
  semantic guarantee, and must be revisited before live TCP framing in Phase 6.

Rollback is source-only: revert the declarative-work module, the additive
Component-kernel field, the effect and subscription mapping hooks, the framed
Layer state, Phase 4 tests, status prose, and this audit packet. No external
resource, persistent state, or live work exists to recover.

## Validation

| Gate | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Pass |
| `cargo test --all-targets` | Pass: 39 passed, 1 intentionally staged/ignored |
| `cargo test --doc` | Pass: 1 runnable doctest and 7 compile-fail contracts |
| `cargo clippy --all-targets -- -D warnings` | Pass |
| `RUSTDOCFLAGS='-D warnings' cargo doc --no-deps` | Pass |
| `git diff --check` | Pass |
| Phase boundary and vocabulary scan | Pass: no Tokio or async calls, Driver invocation, profile binding, scheduling, or Source realization in the Phase 4 kernel or acceptance tests; no governing contract changed |
| Independent read-only audit | GO after two wording corrections; no implementation blocker found |

## Manual Spot Check

Review these in order:

1. `EffectInvocation`, `Command::into_effect`, and
   `Command::into_declarations` in `src/lib.rs` — confirm owned descriptor
   interception remains inert and mapper invocation is structurally at most
   once; exactly-one terminal completion remains Phase 5 runtime work.
2. `src/declarative_work.rs` — confirm reconciliation compares descriptor
   snapshots under one Component owner and does not select a retained mapper.
3. `ComponentKernel::reconcile_subscriptions` — confirm desire is projected
   only from committed Model and no Source is created.
4. `FramedLayer` in `src/lib.rs` — confirm decoder state is deterministic Layer
   mechanism with no Driver or profile dependency.
5. `tests/phase4_declarative_work.rs` — confirm tests map typed data directly
   without pretending terminal execution or Component delivery exists.
6. The V3 and V4 rows in `docs/testing/v0-acceptance-matrix.md` — confirm their
   controlled execution, Source maintenance, and live Driver tranches remain
   staged.
7. `Decoder` and `FramedLayer::map_event` — confirm the accepted deferral: the
   exact EOF/finalization contract remains provisional pending the planned
   `bytes` migration and must be resolved before live TCP framing in Phase 6.

Shane accepted this audit on July 22, 2026. Phase 4 is closed and Phase 5 is
authorized.
