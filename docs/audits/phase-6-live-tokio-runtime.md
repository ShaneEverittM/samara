# Phase 6 Audit: Live Tokio Runtime

- Status: Implementation complete; ready for Shane's audit
- Date: July 23, 2026
- Governing phase: `docs/goal-mode-checklist.md`, Phase 6
- Governing decisions: ADR-0002, ADR-0003, and ADR-0004

> Historical note: ADR-0008 later made every legitimate declared Effect and
> Source binding a synchronous profile-build obligation. The dynamic
> missing-binding scenario and raw API spellings in this phase snapshot are
> intentionally preserved as historical evidence, not current guidance.

## Outcome

The bounded Phase 6 live Tokio runtime is implemented. The same `Program`,
Component, Message, Command, EffectDescriptor, SourceDescriptor, Layer, and Port
contracts used by controlled execution now run against live terminal Drivers.
The implementation owns all Tokio work in one structured runtime scope, closes
admission atomically at shutdown, serializes each Component's transitions, and
does not expose or promise a program-wide order for independent live events.

This phase deliberately uses one runtime-owned dispatcher and unbounded
internal delivery after admission. Those are implementation choices, not API or
conformance requirements. The live contract remains the observable behavior in
ADR-0004: accepted ingress, causal delivery, per-Component non-overlap,
structured Driver ownership, explicit Drain and Cancel cutovers, and no
accidental global ordering promise.

Phase 6 also supplies the first live bridge profile: exact one-shot Tokio
`mpsc` bindings and one-connection TCP byte Sources. TCP emits `bytes::Bytes`;
framing remains a profile-independent Layer whose required `Decoder::finish`
operation makes normal EOF explicit.

## Implemented Slice

### Live assembly and ingress

- `LiveRuntimeBuilder` binds terminal Effect and Source descriptor types,
  exact `StreamDescriptor` values, and the first-party TCP Driver.
- Assembly rejects duplicate or ambiguous bindings and validates knowable
  initial Component work before returning a `LiveRuntime`.
- `LiveRuntime::handle` creates a typed external ingress capability only for a
  Component in that Program. It exposes no Model access.
- A successful `ComponentHandle::send` means accepted into runtime-owned
  delivery. Drain, Cancel, fault, and successful closure reject later ingress.

### Runtime-owned execution

- The live core exclusively owns Component kernels. Messages re-enter that
  owner before transitions, so async work has no direct Model mutation path and
  one Component's transitions cannot overlap.
- Every Command kind is interpreted in the live profile: direct send,
  notification, successful request/reply, timer, finite effect, and batch.
- Effects, Sources, and timers run in a supervised Tokio `JoinSet`. Driver
  results return through a runtime-owned event boundary, and panics are caught
  either at Driver invocation or by task supervision; neither path mutates
  application state from a task.
- Subscription reconciliation preserves Component-local identity, adopts the
  latest mapper for an equal retained descriptor, and gives replacement a new
  private generation. Removal, replacement, shutdown, and fault stop the old
  Source gate and task.
- Composed SourcePlans use the same ordered Layers as controlled execution;
  only the terminal descriptor reaches a live SourceDriver.

### Shutdown and fault boundaries

- `Shutdown::Drain` atomically closes external ingress, stops active Sources,
  disables new Source realization, retains pre-cutoff accepted Source work,
  and recursively processes accepted or causally emitted finite work and
  timers. It has no implicit deadline.
- `Shutdown::Cancel` closes application driving, drops queued semantic work,
  aborts owned tasks, and invokes no application mapper merely because the
  whole scope ended.
- A successful shutdown returns a clean report with zero `remaining`,
  `pending_now`, and `pending_later`. Exact completed/cancelled diagnostics
  remain non-normative.
- Dynamic missing bindings, exhausted one-shot resources, Driver panic, and
  equivalent live mechanism violations fault the scope, close admission, and
  perform the same structured abort cleanup without fabricating typed
  application data.

### Driver and bridge behavior

- A normally returning EffectDriver maps `Succeeded` or `Failed` exactly once.
  Whole-scope cancellation that wins the completion race drops the one-shot
  mapper instead.
- A Source gate serializes successful acceptance order and allows one terminal
  operation.
  The first accepted `end`, `fail`, or implicit normal return wins; later sink
  calls return `DriverStopped`.
- One Tokio `mpsc::Receiver<T>` is consumed by the first activation of its
  exact `StreamDescriptor<T>`. Sender closure ends normally; another claimant
  or later reactivation faults rather than hanging or synthesizing data.
- `TcpBytes` connects once to a numeric `SocketAddr`, emits `bytes::Bytes`,
  reports connect/read failures as typed `TcpError`, maps peer EOF to normal
  ending, closes on cancellation, and contains no retry, reconnect, framing,
  or domain policy.
- `Decoder::finish` runs only for normal underlying end. Successful final
  frames precede outer `Ended`; finalization failure becomes one typed decode
  failure without `Ended`; underlying Source failure is not treated as EOF.

## Acceptance Evidence

| Requirement | Phase 6 evidence |
| --- | --- |
| Admission and serialization | `phase6_live_ingress_success_means_accepted`, `phase6_shutdown_closes_external_ingress`, and `phase6_live_ingress_serializes_component_transitions` prove acceptance, post-cutoff rejection, and non-overlapping serialized transitions under concurrent ingress. |
| Drain | `phase6_drain_stops_sources_and_drains_accepted_causal_work` retains a pre-cutoff Source item through its causally emitted timer and effect while stopping the Source. `phase6_drain_realizes_no_new_sources` proves post-cutoff reconciliation cannot start another Source. The internal `phase6_drain_cutoff_stops_sources_before_queued_backlog` proves the owner observes the atomic phase cutoff and aborts active Sources before driving an already-queued backlog. |
| Cancel | `phase6_cancel_stops_driving_without_mapping_scope_abort` closes the scope before initial work starts. The internal `phase6_cancel_cutoff_starts_no_work_from_a_committed_transition` splits the runtime boundary after a pure transition commits, applies Cancel there without putting synchronization in Component code, and proves its returned Effect is not started; the interpreter also checks the cutoff per batched declaration. `phase6_scope_cancel_drops_effect_without_invoking_mapper` starts a hung Driver, observes its future being dropped, and proves its mapper was not invoked. `phase6_cancel_maps_only_effect_outcomes_accepted_before_cutoff` proves an already-accepted result still consumes its one-shot mapper while queued application driving is discarded. `phase6_source_cancellation_emits_no_unpromised_event` and `phase6_cancel_drops_unanswered_request_without_mapping` cover the other mapper classes. |
| Zero owned work | `phase6_successful_shutdown_owns_zero_work` checks successful Drain and Cancel reports for zero remaining, pending-now, and pending-later work without freezing exact diagnostic counts. `phase6_cancelled_shutdown_future_detaches_no_runtime_work` aborts a host-owned Drain future and proves its retained owner handle still tears down the hung Driver rather than detaching it. |
| Effect completion and binding faults | `phase6_effect_success_and_failure_map_exactly_once` covers one success and one typed failure. `phase6_knowable_live_binding_errors_fail_build` distinguishes assembly errors; `phase6_dynamic_missing_binding_faults_and_cleans_scope` covers a runtime-discovered missing binding and cleanup. |
| Source terminal and cutover | `phase6_source_sink_enqueues_fifo_through_one_terminal` inspects the acceptance boundary directly without relying on downstream Driver scheduling. `phase6_source_delivery_preserves_fifo_through_eof`, `phase6_source_first_terminal_wins`, `phase6_silent_source_return_ends_once`, and `phase6_source_cutover_suppresses_late_terminal` exercise live mapped delivery, explicit and implicit terminal arbitration, and replacement/removal suppression. The internal `phase6_replaced_generation_drops_an_already_accepted_old_event` queues an old-generation event before replacement and proves delivery-time generation validation drops it afterward. |
| Runtime-owned task panic | `phase6_driver_panic_faults_and_cleans_scope` covers an EffectDriver future panic. `phase6_source_driver_panics_fault_and_clean_the_scope` covers both synchronous SourceDriver invocation and its returned future. The internal `phase6_drain_preserves_a_source_panic_completed_before_cutoff` proves Drain cannot hide a join-ready pre-cutoff Source panic, while `phase6_timer_task_panic_is_a_runtime_fault` proves timer-task panic faults instead of leaving a pending timer that can hang Drain. Each becomes a host-visible runtime fault rather than typed application data. |
| Pressure characterization | `phase6_accepted_internal_delivery_has_no_silent_drop` admits and observes 2,048 messages. The count characterizes this implementation only; it establishes no capacity, throughput, latency, or fairness guarantee. |
| Tokio `mpsc` | `phase6_mpsc_closure_ends_once` preserves item FIFO through one normal end. `phase6_mpsc_duplicate_or_reactivation_faults` covers both a second claimant and activation after the one-shot receiver ended. The internal `phase6_mpsc_receiver_is_consumed_at_activation_not_first_poll` proves aborting an unpolled first realization cannot make the unique Receiver reusable. |
| TCP | `phase6_tcp_emits_bytes_and_maps_connect_read_and_peer_eof`, `phase6_tcp_cancellation_closes_connection`, and `phase6_tcp_has_no_hidden_retry_or_framing` cover loopback byte delivery, connect failure, peer EOF, socket ownership, and the deliberately narrow terminal policy. The deterministic internal `phase6_tcp_read_error_maps_to_typed_source_failure` drives the production read loop with a failing reader and proves one typed read failure. `phase6_tcp_failure_is_constructible_and_scriptable_in_controlled_execution` proves controlled tests can supply the same public typed failure. |
| Decoder EOF | `phase6_framed_eof_emits_final_frames_before_ended`, `phase6_framed_finish_failure_emits_failed_without_ended`, and `phase6_framed_source_failure_does_not_run_finish` cover normal finalization, finalization failure, underlying failure, and earlier decode failure. The internal live `phase6_drain_does_not_finalize_after_an_earlier_decode_failure` queues a decode-failing item and EOF before Drain, then proves the earlier terminal failure suppresses finalization. |
| Request causality | `phase6_live_request_reply_drains_causally` proves a typed Request, provider transition, Reply, requester continuation, and causal finite work all finish under Drain. `phase6_cancel_drops_unanswered_request_without_mapping` covers scope abort. |
| Live/controlled parity | `v5_live_and_controlled_boundaries_map_equivalent_observations` compares the same typed success/failure observations across profiles rather than scheduler steps. All three reference examples compile the same program factory for both profiles and run live and controlled smoke paths. |
| No global live order | `v8_independent_live_events_accept_either_order` deliberately forces both valid completion orders. `v8_conformance_compares_partial_order_not_scheduler_sequence` accepts both while checking the shared causal obligations. |
| Shallow onboarding | `v11_minimal_component_runs_with_live_mpsc` runs the minimal Component through the first-party bridge; its controlled counterpart uses the unchanged Component and program factory. |

## Topology and Ordering Audit

The implementation currently uses one Tokio owner task for runtime dispatch and
a supervised task set for live Drivers and timers. No public item names that
dispatcher as a mailbox, actor, central event loop, worker, or scheduler
guarantee. Components cannot inspect its queues, task identity, or event order.

The owner naturally serializes all transitions today, which is stronger than
the required per-Component non-overlap internally. The public contract and
acceptance assertions do not depend on that extra serialization. Independent
effect tests force both orders and compare causal partial order, so a future
per-Component, sharded, or hybrid implementation can conform without changing
application code or tests.

The Source gate supplies the narrow sequencing ADR-0004 actually promises:
successful sink calls enqueue before returning, preserve their serialized
acceptance order within one generation, and admit at most one terminal
operation. Private generation checks
then enforce hard cutover independently of scheduler topology.

## Invariant Impact

- `Model` remains runtime-owned. Neither ComponentHandle, Driver future,
  SourceSink, timer task, nor reply capability can access or mutate it.
- `Message` remains the only state-transition input. Effect outcomes, Source
  events, replies, timers, notifications, and external ingress all become
  typed Messages before a Component transition.
- `update` remains synchronous and side-effect free by contract. The live core
  interprets its returned Command only after the transition finishes.
- EffectDescriptor and SourceDescriptor values remain inert typed data.
  World-facing execution occurs only after terminal live binding selection.
- Layers remain deterministic, runtime-owned composition that is shared by
  live and controlled profiles. Drivers remain terminal mechanism only.
- Per-Component transitions are serialized; causal edges and promised
  per-Source sequencing are preserved; independent live events acquire no
  program-wide order promise.
- Controlled logical time, structural tracing, and deterministic scheduling are
  unchanged. Live execution adds no public observer or feedback path.

## Failure Modes and Recovery

- **Drain never returns:** a hung finite Driver, unanswered Request,
  far-future/recurring timer, or application that recursively emits finite
  work can keep Drain open forever. The host chooses Cancel or applies its own
  deadline; Samara does not silently escalate.
- **Unbounded delivery exhausts memory:** after admission, v0 does not apply an
  internal capacity or shedding policy. Recovery is host/process recovery;
  capacity, quotas, and overload controls remain deferred.
- **Missing or ambiguous binding:** knowable initial/duplicate bindings fail
  assembly. A descriptor reached only dynamically faults the running scope.
  Recovery is to correct assembly and start a fresh runtime.
- **One-shot `mpsc` exhaustion:** duplicate activation or reactivation faults
  because no second receiver exists and `Infallible` cannot carry a truthful
  in-band failure. Recovery is a fresh receiver and runtime assembly.
- **Driver panic or mechanism error:** the whole v0 scope faults, ingress
  closes, owned tasks are cancelled, and no fabricated application outcome is
  emitted. Recovery/restart and narrower isolation remain deferred.
- **Source replacement/removal race:** the cutover gate and private generation
  suppress old work before transition. The application receives no synthetic
  terminal event for cancellation or replacement.
- **Scope cancellation race:** one admission boundary decides whether a Driver
  result maps exactly once or its future and mapper are dropped. Cancel itself
  is not application data.
- **Non-conforming application code:** Components, mappers, Layers, and
  descriptor implementations can still panic or perform hidden ambient work.
  Runtime-owned Driver panics are contained, but Rust's type system does not
  make arbitrary user code pure. Conformance remains documented user
  responsibility.

No Phase 6 runtime state is durable. Rollback is source-only: revert the live
core, live façade implementation, first-party bindings, Decoder finalization,
Phase 6 tests/reference-profile activation, and this audit packet. There is no
persistent data migration or replay recovery step.

## Preserved Deferrals

- Bounded queues, quotas, shedding, coalescing, fairness, priorities, stable
  throughput/latency targets, and overload metrics.
- Shutdown deadlines, grace periods, escalation, readiness, host-signal policy,
  and normative completed/cancelled report counts.
- Request failure, timeout, abandonment, late Reply, delegation, and in-band
  cancellation beyond the accepted successful path and whole-scope abort.
- Notification delivery-failure policy and per-effect deadline, supersession,
  or application-visible cancellation policy.
- Automatic Source retry/restart, shared or restartable channel bridges, DNS,
  connection policy, and Driver isolation/recovery.
- General public Layer/profile-binding abstractions and a mature first-party
  Tokio bridge module organization.
- Public live tracing/observation, payload capture, typed trace projections,
  streaming observers, durable trace storage, persistence, and replay.

## Validation

| Gate | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Pass |
| `cargo test --all-targets` | Pass: 126 tests, including 25 live-runtime integration tests, 6 bridge tests, 26 library tests, and live/controlled tests for all three examples |
| `cargo test --doc` | Pass: 9 doctests |
| `cargo clippy --all-targets --all-features -- -D warnings` | Pass |
| `RUSTDOCFLAGS='-D warnings' cargo doc --no-deps --all-features` | Pass |
| `git diff --check` | Pass |
| `cargo run --example minimal`, `api_pressure`, and `framed_socket` | Pass: all three example binaries build and run; their test targets separately exercise both runtime profiles |
| Governing-contract scan | No ADR or accepted semantic rule changed; Phase 6 implements ADR-0004's bounded slice |
| Topology scan | No public queue, dispatcher, task, worker, or independent-event total order exposed |
| Public observer scan | No public live trace, callback, scheduler hook, or Component-readable observer added |

## Manual Spot Check

Review these in order:

1. `LiveScope::accept_ingress`, `begin_shutdown`, and `fault` in
   `src/live_runtime.rs` — confirm send success is admission, all ending modes
   close ingress, and no independent scheduler sequence becomes API.
2. `LiveCore::run`, `process_message`, and `interpret_command` — confirm the
   runtime owner is the only Component transition caller and all async results
   re-enter as Messages.
3. `begin_drain`, `drain_finished`, and `cleanup_abort` — confirm Drain retains
   accepted/causal finite work while stopping Sources, whereas Cancel/fault
   abort tasks without invoking application mappers.
4. `activate_source`, `apply_subscription_changes`, `SourceGate`, and
   `process_source_event` — confirm one terminal winner, FIFO enqueue,
   retained-mapper adoption, hard generation cutover, and stale-work rejection.
5. `spawn_owned` and Driver task-exit handling — confirm every runtime-owned
   future is supervised, panic becomes RuntimeError, and no Driver task detaches
   after successful shutdown.
6. `LiveBindings` and `LiveRuntimeBuilder::build` — confirm duplicate/ambiguous
   and knowable missing bindings fail assembly while dynamic violations fault
   the running scope.
7. `MpscBinding` and `TokioTcpDriver` — confirm the one-shot receiver and
   one-connection byte Source contain no application retry, reconnect,
   framing, or overload policy.
8. `Decoder::finish`, `FramedLayer::map_event`, and the Phase 6 EOF tests —
   confirm only normal EOF finalizes and final frames cannot be overtaken by
   `Ended`.
9. `tests/phase6_live_runtime.rs` and `tests/phase6_bridges.rs` — confirm tests
   assert observable semantics and causal partial order, not task layout or one
   live event vector.
10. The live and controlled tests in all three examples — confirm each pair
    shares one Component/program factory and differs only in runtime decisions
    and terminal-world bindings.

Shane's manual audit and Phase 6 acceptance remain open. Do not advance to
Phase 7 until he accepts this packet.
