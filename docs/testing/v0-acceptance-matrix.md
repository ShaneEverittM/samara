# Samara v0 Acceptance Matrix

- Status: Phase 6 live-runtime contract accepted; scenarios active
- Date: July 23, 2026
- Scope: Traceability from vision requirements to topology-neutral evidence

## How to Read This Matrix

This matrix names the acceptance scenarios before implementation begins. It
does not make future behavior pass by ignoring a `todo!` test. A scenario has
one of three states:

- **Active** — executable in the current phase and required to pass.
- **Staged** — specified now and activated as a failing test before its listed
  implementation phase begins.
- **Decision gate** — blocked by an explicitly deferred semantic choice. The
  owning phase must settle or further scope the choice before implementation.

Phase 2 established the scenario names, topology-neutral assertions, and
ownership by phase. A later approved contract may refine them before activation;
[ADR-0003](../adr/0003-controlled-execution-semantics.md) records the accepted
Phase 5 refinements. Accepted
[ADR-0004](../adr/0004-initial-live-runtime-semantics.md) specifies the active
Phase 6 tranche below. Exact harness APIs may be chosen with the phase that
first needs them where the accepted contract does not freeze their spelling.

## Vision Traceability

| Vision requirement | Named scenarios | Current contract evidence | Activation |
| --- | --- | --- | --- |
| V1 Repeatable Component Transition | `v1_same_input_produces_equivalent_model_and_command_intent`; `v1_equivalent_effect_outcomes_produce_equivalent_messages`; `v1_update_has_no_runtime_capability` | Direct-transition, typed-intent, controlled mapper, and compile-contract evidence are active. Phase 6 repeats outcome mapping through live Drivers. | Phase 3, completed controlled in Phase 5 and repeated live in Phase 6 |
| V2 Isolated State Ownership | `v2_same_component_transitions_never_overlap`; `v2_live_handle_cannot_mutate_model`; `v2_cross_component_interaction_reenters_as_message`; `v2_protocol_message_remains_distinct_from_provider_message` | Component/handle type separation, controlled delivery, and Protocol conversion are active. Phase 6 repeats serialization through admitted live ingress. | Phase 3 and Phase 5; active live in Phase 6 |
| V3 Interceptable Effect Descriptor | `v3_effect_command_exposes_typed_descriptor`; `v3_opaque_async_work_is_not_an_effect_descriptor`; `v3_controlled_effect_maps_exactly_once_without_live_driver` | Typed descriptor inspection, compile-fail API constraints, and controlled interpretation are active. Live Driver interpretation is active for Phase 6. | Phase 4, completed controlled in Phase 5 and active live in Phase 6 |
| V4 Declarative Subscription Lifecycle | `v4_subscriptions_are_pure_model_projection`; `v4_new_identity_starts_source`; `v4_equal_descriptor_retains_source_and_adopts_latest_mapper`; `v4_removed_identity_cancels_source`; `v4_changed_descriptor_replaces_source`; `v4_replaced_generation_discards_stale_work`; `v4_composed_source_plan_reaches_terminal_controlled_behavior`; `v4_controlled_events_use_reusable_mapper`; `v4_source_failure_enters_as_message` | Projection, reconciliation, retained-mapper, hard-cutover, SourcePlan, and controlled lifecycle behavior are active. ADR-0004 adds active live terminal-arbitration and cancellation evidence. | Phase 4 and Phase 5; active live in Phase 6 |
| V5 Live and Controlled Program Parity | `v5_one_program_factory_compiles_for_both_profiles`; `v5_reference_components_are_profile_invariant`; `v5_live_and_controlled_boundaries_map_equivalent_observations` | The reference examples compile one program factory into both assembly shapes. Behavioral parity is active under ADR-0004. | Phase 6 |
| V6 Program-Wide Controlled Determinism | `v6_identical_controlled_runs_have_equal_structural_trace_and_final_state`; `v6_equal_final_state_with_different_structural_trace_is_not_equivalent`; `v6_typed_descriptor_payload_is_compared_by_direct_intent_test` | ADR-0003 defines structural trace equivalence and keeps typed domain-payload comparison in direct intent tests rather than requiring generic trace payload capture. | Phase 5 |
| V7 Controlled Time Progression | `v7_manual_advance_uses_no_wall_sleep`; `v7_automatic_advance_reaches_next_instant`; `v7_equal_time_uses_deterministic_causal_insertion_order`; `v7_equivalent_schedules_are_repeatable`; `v7_initial_work_uses_component_id_not_registration_order` | ADR-0003 fixes the v0 scheduler key as logical deadline plus deterministic insertion ticket, and the active Phase 5 runtime exercises every listed scenario. | Phase 5 |
| V8 Causal Semantics Without Global Order | `v8_causal_edges_are_preserved`; `v8_component_transitions_do_not_overlap`; `v8_independent_live_events_accept_either_order`; `v8_conformance_compares_partial_order_not_scheduler_sequence` | Controlled causal edges and Component serialization are active. ADR-0004 requires tests to exercise both valid independent live orders and compare causal partial order rather than a trace vector. | Phase 3 serialization, Phase 5 controlled causality, active Phase 6 live independence, final audit Phase 7 |
| V9 Structured Work Ownership | `v9_drive_reports_semantic_pending_work`; `v9_pending_now_counts_ready_messages_and_due_timers`; `v9_pending_later_counts_effects_future_timers_sources_and_requests`; `v9_controlled_cancel_leaves_zero_work`; `v9_live_shutdown_leaves_zero_work`; `v9_effect_cancellation_has_one_outcome`; `v9_scope_cancel_maps_no_application_outcome`; `v9_source_cancellation_emits_no_unpromised_event` | ADR-0003 defines controlled obligations and explicit controlled effect cancellation. ADR-0004 distinguishes whole-scope abort, which maps no application outcome, and requires zero live work after successful shutdown. | Phase 5 controlled; active Phase 6 live; final audit Phase 7 |
| V10 Non-Influential Semantic Trace | `v10_reading_controlled_trace_after_drive_has_no_feedback_path`; `v10_trace_explains_transitions_work_time_and_causation`; `v10_trace_records_stale_source_drops` | ADR-0003 requires always-collected, read-afterward controlled records with logical time, parentless roots, and exactly one immediate parent for every non-root. ADR-0004 explicitly defers a public live observer beyond v0. | Phase 5 controlled; public live observer deferred beyond v0 |
| V11 Shallow Tokio Onboarding | `v11_minimal_tokio_mpsc_application_compiles`; `v11_minimal_component_runs_with_controlled_stream`; `v11_minimal_component_runs_with_live_mpsc` | The minimal example and its controlled runtime test are active. The live one-shot `mpsc` path is active under ADR-0004. | Phase 5 controlled; active Phase 6 live |

## Active Phase 2 Evidence

The following evidence must pass before the Phase 2 audit:

- `tests/phase2_contract.rs` exercises V1 direct transition equivalence, V3
  typed effect inspection, V4 subscription identity/descriptor separation, and
  a non-`Sync` Component configuration accepted by the kernel contract.
- `examples/minimal.rs` compiles the V11 onboarding shape and directly tests its
  transition and subscription projection.
- `examples/api_pressure.rs` compiles handwritten Protocol/Request, Port,
  EffectDescriptor, timer, live assembly, and controlled assembly shapes.
- `examples/framed_socket.rs` compiles the Layer/Driver, generated Protocol,
  provider conversion, request-continuation, and subscription-replacement
  shapes. Its direct tests cover the pure decoder and command intent.
- Library unit tests validate the `protocol!` expansion and typed Request reply
  association.
- Rustdoc compile-fail cases validate that an arbitrary value cannot be passed
  as an EffectDescriptor, an async closure cannot replace a synchronous message
  mapper, and discarding `ReplyTo` violates its `#[must_use]` contract when the
  lint is denied.

The demanding controlled example functions that were staged in Phase 2 became
active tests in Phase 5. Their live counterparts are active for Phase 6.

## Topology-Neutral Assertion Rules

Acceptance tests may assert:

- Component identity and non-overlapping transitions.
- Model, Message, Command intent, desired Subscription, and typed outcome/event
  semantics.
- Causal edges and sequencing explicitly promised by a contract.
- Logical time, pending-work classification, final state, and structured
  semantic trace.
- Either completion order for independent live events.
- The ADR-0003 equal-time order in controlled execution, while treating it as
  reproducibility machinery rather than a cross-profile domain guarantee.

They must not assert:

- A mailbox, actor task, central loop, worker count, queue type, queue position,
  Tokio task ID, thread ID, or registration order.
- A total order between causally independent live events.
- Closure object identity or internal type-erasure layout.
- That observing one scheduler order proves the absence of a global-order
  dependency.
- Registration order as the ordering of initial controlled work.

V8 evidence therefore compares a causal partial order and deliberately runs
independent events in more than one valid order. It never blesses one observed
interleaving as the program contract.

## Structured-Ownership Observation Points

V9's phrase “at every observation point” means these bounded public points:

1. When a controlled drive operation returns.
2. When controlled execution is quiescent now but awaits input or logical time.
3. Immediately before and after a scripted EffectOutcome or SourceEvent is
   accepted.
4. When controlled cancellation returns.
5. When live structured shutdown returns.

The runtime may maintain finer internal accounting, but the conformance suite
does not require arbitrary inspection during an in-progress transition.

At these points, `pending_now` counts accepted Component Messages and due
timers. `pending_later` counts pending effects, future timers, active Sources
(one each), and outstanding Requests. Runtime tasks, queues, locks,
interpreter steps, and trace records are not additional units. Request delivery
may temporarily contribute both an outstanding Request and a queued provider
Message. Controlled cancellation must reduce both counts to zero.

## Phase 5 Contract Scenarios

The approved Phase 5 tranche additionally requires:

- `phase5_program_build_rejects_duplicate_component_id`
- `phase5_program_build_rejects_duplicate_protocol_and_port_id`
- `phase5_program_build_requires_exactly_one_binding_per_port`
- `phase5_program_build_rejects_provider_from_another_builder`
- `phase5_program_build_allows_port_cycles`
- `phase5_missing_controlled_behavior_faults_without_live_fallback`
- `phase5_faulted_run_remains_inspectable_and_cancellable`
- `phase5_successful_request_reply_is_correlated_at_most_once`
- `phase5_unanswered_request_remains_pending_until_cancellation`

The successful Request path maps one Reply to
`RequestOutcome::Replied`, records its causal chain, and counts one outstanding
Request. Phase 5 does not synthesize `Failed`, `TimedOut`, or `Cancelled`.

## Resolved Phase 5 Decision Gates

ADR-0003 resolves the former Phase 5 blockers:

- A retained Source uses the latest post-transition mapper.
- Source replacement is a hard private-generation cutover; stale undelivered
  work is discarded and traced.
- Composed Sources automatically lower to runtime-owned SourcePlans and only
  terminal descriptors receive profile bindings.
- The controlled equal-time tie-break is deterministic causal insertion order.
- The controlled trace is always-collected, structural, causal, logical-time
  stamped, in-memory, and read after driving.
- `ProgramBuilder::build()` is the fallible boundary for explicitly knowable
  assembly errors only.
- Missing controlled terminal behavior faults without invoking a live Driver
  or message mapper.
- Pending work counts the semantic obligations defined above.
- Phase 5 request/reply scope is the successful path only.

## Active Phase 6 Contract Scenarios

Shane accepted ADR-0004; these scenarios are now Active and required for the
Phase 6 audit.

Admission and shutdown:

- `phase6_live_ingress_success_means_accepted`
- `phase6_shutdown_closes_external_ingress`
- `phase6_live_ingress_serializes_component_transitions`
- `phase6_drain_stops_sources_and_drains_accepted_causal_work`
- `phase6_drain_realizes_no_new_sources`
- `phase6_cancel_stops_driving_without_mapping_scope_abort`
- `phase6_successful_shutdown_owns_zero_work`

Driver lifecycle and faults:

- `phase6_knowable_live_binding_errors_fail_build`
- `phase6_dynamic_missing_binding_faults_and_cleans_scope`
- `phase6_effect_success_and_failure_map_exactly_once`
- `phase6_scope_cancel_drops_effect_without_invoking_mapper`
- `phase6_source_first_terminal_wins`
- `phase6_silent_source_return_ends_once`
- `phase6_source_cutover_suppresses_late_terminal`
- `phase6_source_delivery_preserves_fifo_through_eof`
- `phase6_driver_panic_faults_and_cleans_scope`

Pressure and first-party bridges:

- `phase6_accepted_internal_delivery_has_no_silent_drop`
- `phase6_mpsc_closure_ends_once`
- `phase6_mpsc_duplicate_or_reactivation_faults`
- `phase6_tcp_emits_bytes_and_maps_connect_read_and_peer_eof`
- `phase6_tcp_cancellation_closes_connection`
- `phase6_tcp_has_no_hidden_retry_or_framing`
- `phase6_framed_eof_emits_final_frames_before_ended`
- `phase6_framed_finish_failure_emits_failed_without_ended`
- `phase6_framed_source_failure_does_not_run_finish`

Parity and ordering:

- `v5_live_and_controlled_boundaries_map_equivalent_observations`
- `v8_independent_live_events_accept_either_order`
- `v8_conformance_compares_partial_order_not_scheduler_sequence`
- `v11_minimal_component_runs_with_live_mpsc`

The load scenario is characterization only. It must record workload and result
without turning queue capacity, throughput, tail latency, or fairness into a
stable conformance threshold. Tests of explicit controlled
`EffectOutcome::Cancelled` remain valid: ADR-0004's no-mapper rule
applies specifically to aborting the whole live runtime scope.

## Remaining Deferrals

- Descriptor/message payload capture, typed trace projections, a public live
  observer, durable trace storage, and replay. In particular,
  `v10_live_observer_has_no_feedback_path` is deferred beyond final v0
  conformance rather than staged for Phase 6.
- Request failure, timeout, abandonment, late-Reply, delegation, and in-band
  cancellation semantics.
- Notification delivery failure.
- Bounded pressure, overload controls, shutdown deadlines and escalation,
  exact shutdown diagnostic counts, per-effect cancellation policy, Driver
  recovery, automatic Source retry, and restartable or shared bridges beyond
  ADR-0004's simple first cut.
- Exact public first-party module/type naming and general live Layer/profile
  binding abstractions where no accepted API already freezes them.

No implementation goal may choose a deferred policy merely to make a test
green. The owning phase first updates the governing contract and acceptance
scenario, then receives human approval.

## Final Gate

Phase 7 activates the complete matrix and runs it as the topology-independent
conformance suite. Persistence and durable replay remain outside v0 and have no
scenario in this matrix.
