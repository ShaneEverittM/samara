# Samara v0 Acceptance Matrix

- Status: Phase 6 live-runtime implementation complete; ADR-0008 and ADR-0009 scenarios active
- Date: July 25, 2026
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
Accepted [ADR-0006](../adr/0006-live-port-ingress.md) specifies the active live
Port-ingress tranche. Its first executable slice was written red before the
implementation and now exercises the central routing, correlation, and
lifecycle contract. Accepted
[ADR-0007](../adr/0007-live-host-lifecycle.md) specifies cancellation-safe
terminal owner observation and its composition with host shutdown futures.
Accepted [ADR-0008](../adr/0008-closed-program-capabilities.md) supersedes the
earlier open Effect/Source dependency qualification: both profiles now validate
the complete Program-declared `EffectCapability<D>` and `SourceCapability<S>`
set before execution. Accepted
[ADR-0009](../adr/0009-first-party-stdin-lines.md) defines the first-party
standard-input line Source, its exact Unix live binding, and structured reader
cutover.

## Vision Traceability

| Vision requirement | Named scenarios | Current contract evidence | Activation |
| --- | --- | --- | --- |
| V1 Repeatable Component Transition | `v1_same_input_produces_equivalent_model_and_command_intent`; `v1_equivalent_effect_outcomes_produce_equivalent_messages`; `v1_update_has_no_runtime_capability` | Direct-transition, typed-intent, controlled mapper, and compile-contract evidence are active. Phase 6 repeats outcome mapping through live Drivers. | Phase 3, completed controlled in Phase 5 and repeated live in Phase 6 |
| V2 Isolated State Ownership | `v2_same_component_transitions_never_overlap`; `v2_live_handle_cannot_mutate_model`; `v2_live_port_handle_cannot_mutate_model`; `v2_cross_component_interaction_reenters_as_message`; `v2_protocol_message_remains_distinct_from_provider_message` | Component/handle type separation, controlled delivery, and Protocol conversion are active. Phase 6 repeats serialization through admitted live Component and provider-neutral Port ingress. | Phase 3 and Phase 5; implemented live in Phase 6, final Port-ingress audit pending |
| V3 Interceptable Effect Descriptor | `v3_effect_command_exposes_typed_descriptor`; `v3_opaque_async_work_is_not_an_effect_descriptor`; `v3_raw_effect_descriptor_cannot_issue_command`; `v3_controlled_effect_maps_exactly_once_without_live_driver` | Typed descriptor and capability inspection, compile-fail API constraints, and controlled interpretation are active. Live Driver interpretation is implemented for Phase 6. | Phase 4, completed controlled in Phase 5 and implemented live in Phase 6, extended by ADR-0008 |
| V4 Declarative Subscription Lifecycle | `v4_subscriptions_are_pure_model_projection`; `v4_new_identity_starts_source`; `v4_equal_descriptor_retains_source_and_adopts_latest_mapper`; `v4_equal_descriptor_with_another_capability_replaces_source`; `v4_removed_identity_cancels_source`; `v4_changed_descriptor_replaces_source`; `v4_replaced_generation_discards_stale_work`; `v4_composed_source_plan_reaches_terminal_controlled_behavior`; `composed_source_capability_declares_only_its_terminal_requirement`; `v4_controlled_events_use_reusable_mapper`; `v4_source_failure_enters_as_message` | Projection, capability-aware reconciliation, retained-mapper, hard-cutover, SourcePlan, and controlled lifecycle behavior are active. ADR-0004's live terminal-arbitration and cancellation evidence is implemented. | Phase 4 and Phase 5; implemented live in Phase 6 and extended by ADR-0008 |
| V5 Live and Controlled Program Parity | `v5_one_program_factory_compiles_for_both_profiles`; `v5_reference_components_are_profile_invariant`; `v5_live_and_controlled_boundaries_map_equivalent_observations` | The reference examples declare one closed Program capability set and use the same Component configuration in both profile assembly shapes. Behavioral parity evidence is implemented under ADR-0004. | Phase 6, extended by ADR-0008 |
| V6 Program-Wide Controlled Determinism | `v6_identical_controlled_runs_have_equal_structural_trace_and_final_state`; `v6_equal_final_state_with_different_structural_trace_is_not_equivalent`; `v6_typed_descriptor_payload_is_compared_by_direct_intent_test` | ADR-0003 defines structural trace equivalence and keeps typed domain-payload comparison in direct intent tests rather than requiring generic trace payload capture. | Phase 5 |
| V7 Controlled Time Progression | `v7_manual_advance_uses_no_wall_sleep`; `v7_automatic_advance_reaches_next_instant`; `v7_equal_time_uses_deterministic_causal_insertion_order`; `v7_equivalent_schedules_are_repeatable`; `v7_initial_work_uses_component_id_not_registration_order` | ADR-0003 fixes the v0 scheduler key as logical deadline plus deterministic insertion ticket, and the active Phase 5 runtime exercises every listed scenario. | Phase 5 |
| V8 Causal Semantics Without Global Order | `v8_causal_edges_are_preserved`; `v8_component_transitions_do_not_overlap`; `v8_independent_live_events_accept_either_order`; `v8_live_port_request_preserves_request_reply_causality`; `v8_conformance_compares_partial_order_not_scheduler_sequence` | Controlled causal edges and Component serialization are active. Phase 6 tests exercise both valid independent live orders and reverse-completion Port request correlation without ordering independent handle calls. | Phase 3 serialization, Phase 5 controlled causality, implemented Phase 6 live independence and Port ingress, final audit Phase 7 |
| V9 Structured Work Ownership | `v9_drive_reports_semantic_pending_work`; `v9_pending_now_counts_ready_messages_and_due_timers`; `v9_pending_later_counts_effects_future_timers_sources_and_requests`; `v9_controlled_cancel_leaves_zero_work`; `v9_live_shutdown_leaves_zero_work`; `v9_effect_cancellation_has_one_outcome`; `v9_scope_cancel_maps_no_application_outcome`; `v9_source_cancellation_emits_no_unpromised_event`; `v9_dropped_host_waiter_does_not_cancel_request`; `run_forever_surfaces_dynamic_foreign_capability_before_live_driver`; `cancelled_run_forever_observation_preserves_owner_for_explicit_shutdown`; `dropping_idle_stdin_reader_interrupts_and_joins_its_thread` | ADR-0003 defines controlled obligations and explicit controlled effect cancellation. Phase 6 distinguishes whole-scope abort from application outcomes, proves that dropping a host waiter cannot abandon admitted runtime work, and exposes owner termination without making observation cancellation an ownership cutoff. ADR-0009 proves that even an idle blocking stdin boundary remains interruptible and joined. | Phase 5 controlled; implemented Phase 6 live, Port ingress, host lifecycle, ADR-0008 provenance, and ADR-0009 stdin ownership; final audit Phase 7 |
| V10 Non-Influential Semantic Trace | `v10_reading_controlled_trace_after_drive_has_no_feedback_path`; `v10_trace_explains_transitions_work_time_and_causation`; `v10_trace_records_stale_source_drops` | ADR-0003 requires always-collected, read-afterward controlled records with logical time, parentless roots, and exactly one immediate parent for every non-root. ADR-0004 explicitly defers a public live observer beyond v0. | Phase 5 controlled; public live observer deferred beyond v0 |
| V11 Shallow Tokio Onboarding | `v11_minimal_tokio_mpsc_application_compiles`; `v11_minimal_component_runs_with_controlled_stream`; `v11_minimal_component_runs_with_live_mpsc`; `stdin_descriptor_and_errors_are_typed_fixture_data`; `controlled_stdin_delivers_lines_and_normal_eof`; `stdin_reader_delivers_framed_lines_then_eof_from_injected_stream` | The minimal example runs unchanged through controlled stream input and the live one-shot `mpsc` bridge. The expanded application uses the first-party `StdinLines` boundary without a host-owned reader thread or channel adapter. | Phase 5 controlled; implemented Phase 6 live and extended by ADR-0009 |
| V12 Closed Program Capability Assembly | `later_effect_dependency_fails_live_and_controlled_build_when_unbound`; `later_source_dependency_fails_live_and_controlled_build_when_unbound`; `raw_descriptors_cannot_issue_work`; `initially_visible_foreign_effect_capability_is_rejected_during_profile_build`; `dynamically_used_foreign_effect_capability_is_rejected_before_controlled_behavior`; `dynamically_used_foreign_effect_capability_is_rejected_before_live_driver`; `exact_stream_binding_rejects_a_foreign_capability_during_build`; `two_exact_stream_capabilities_of_one_type_bind_independently`; `exact_stream_input_is_disambiguated_by_capability_not_equal_descriptor`; `composed_source_capability_declares_only_its_terminal_requirement`; `live_stdin_binding_is_exact_and_required`; `live_stdin_binding_accepts_a_composed_capability`; `live_stdin_rejects_duplicate_or_competing_process_bindings` | ADR-0008 compile contracts and `closed_program_capabilities` scenarios cover closed issuance, complete profile validation, provenance, exact identity, and composed terminal declaration. ADR-0009 extends exact binding evidence to the unique Unix process-input resource. | ADR-0008 and ADR-0009 |

## Active Phase 2 Evidence

The following evidence must pass before the Phase 2 audit:

- `tests/phase2_contract.rs` exercises V1 direct transition equivalence, V3
  typed effect inspection, V4 subscription identity/descriptor separation, and
  a non-`Sync` Component configuration accepted by the kernel contract.
- `examples/minimal/src/main.rs` compiles the V11 onboarding shape and directly
  tests its transition and subscription projection.
- `examples/api_pressure/src/main.rs` compiles handwritten Protocol/Request, Port,
  EffectDescriptor, timer, live assembly, and controlled assembly shapes.
- `examples/framed_socket/src/main.rs` compiles the Layer/Driver, generated Protocol,
  provider conversion, request-continuation, and subscription-replacement
  shapes. Its direct tests cover the pure decoder and command intent.
- Library unit tests validate the `protocol!` expansion and typed Request reply
  association.
- Rustdoc compile-fail cases validate that an arbitrary value cannot be passed
  as an EffectDescriptor, an async closure cannot replace a synchronous message
  mapper, raw descriptors cannot bypass Program-issued Effect/Source
  capabilities, and discarding either `ReplyTo` or `Command` violates its
  `#[must_use]` contract when the lint is denied.
- Discarded-outcome conformance tests prove controlled outcomes remain traced
  without scheduling a Message and live Drain/Cancel retain structured
  ownership of the effect.

The demanding controlled example functions that were staged in Phase 2 became
active tests in Phase 5. Their live counterparts are implemented and passing
for Phase 6, pending Shane's audit.

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
6. When live terminal owner observation returns after structured cleanup.

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
- `phase5_missing_controlled_behavior_is_rejected_without_live_fallback`
- `phase5_missing_controlled_behavior_rejects_before_any_run_exists`
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
- `ProgramBuilder::build()` is the fallible boundary for logical assembly
  errors. ADR-0008 subsequently closes its issued capability set.
- ADR-0008 supersedes runtime discovery of legitimate missing controlled
  behavior: every declaration must be satisfied at controlled profile build.
  A hidden foreign capability still faults before behavior or a mapper.
- Pending work counts the semantic obligations defined above.
- Phase 5 request/reply scope is the successful path only.

## Active ADR-0008 Closed-Capability Scenarios

The `closed_program_capabilities` suite and capability-aware reconciliation
tests require:

- `later_effect_dependency_fails_live_and_controlled_build_when_unbound`
- `later_source_dependency_fails_live_and_controlled_build_when_unbound`
- `declared_source_runs_in_controlled_execution`
- `declared_effect_runs_in_controlled_execution`
- `declared_effect_runs_in_live_execution`
- `initially_visible_foreign_effect_capability_is_rejected_during_profile_build`
- `initially_visible_foreign_source_capability_is_rejected_during_profile_build`
- `initially_visible_foreign_component_and_port_capabilities_fail_profile_build`
- `dynamically_used_foreign_effect_capability_is_rejected_before_controlled_behavior`
- `run_forever_surfaces_dynamic_foreign_capability_before_live_driver`
- `dynamically_reached_foreign_source_faults_before_terminal_behavior`
- `exact_stream_binding_rejects_a_foreign_capability_during_build`
- `two_exact_stream_capabilities_of_one_type_bind_independently`
- `controlled_exact_stream_rejects_duplicate_and_type_wide_ambiguity`
- `exact_stream_input_is_disambiguated_by_capability_not_equal_descriptor`
- `live_exact_streams_with_equal_descriptors_route_by_capability`
- `composed_source_capability_declares_only_its_terminal_requirement`
- `exact_stream_bridge_accepts_a_composed_source_capability`
- `live_exact_stream_bridge_runs_composed_layers_before_component_mapping`
- `v4_equal_descriptor_with_another_capability_replaces_source`

Rustdoc compile-fail evidence additionally proves that a raw EffectDescriptor
cannot construct an Effect Command and a raw SourceDescriptor cannot construct
a Subscription. First-party HTTP, standard-output, and stream tests exercise
the same capability path so convenience APIs cannot become bypasses.

These scenarios assert synchronous profile-build rejection for every
legitimate declared missing binding. The later foreign-capability scenario is
deliberate non-conforming code and must fault before controlled behavior; it is
not dynamic dependency discovery.

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
- `phase6_declared_missing_binding_rejects_live_build`
- `phase6_effect_success_and_failure_map_exactly_once`
- `phase6_scope_cancel_drops_effect_without_invoking_mapper`
- `phase6_source_first_terminal_wins`
- `phase6_silent_source_return_ends_once`
- `phase6_source_cutover_suppresses_late_terminal`
- `phase6_source_delivery_preserves_fifo_through_eof`
- `phase6_driver_panic_faults_and_cleans_scope`

Standard `From` continuations:

- `effect_default_uses_from_and_effect_with_preserves_context`
- `request_default_uses_from_and_request_with_preserves_context`
- `source_default_is_reusable_and_source_with_preserves_context`
- `http_pipeline_default_uses_from_and_into_command_with_is_explicit`

Pressure and first-party bridges:

- `phase6_accepted_internal_delivery_has_no_silent_drop`
- `phase6_mpsc_closure_ends_once`
- `phase6_mpsc_duplicate_or_reactivation_faults`
- `phase6_tcp_emits_bytes_and_maps_connect_read_and_peer_eof`
- `phase6_tcp_cancellation_closes_connection`
- `phase6_tcp_has_no_hidden_retry_or_framing`
- `http_descriptor_preserves_method_url_headers_and_owned_body`
- `controlled_http_is_an_ordinary_typed_effect_boundary`
- `live_http_preserves_raw_status_and_reuses_one_client_pool`
- `invalid_live_http_configuration_is_a_typed_effect_failure`
- `live_http_eof_is_a_typed_transport_failure`
- `bind_http_participates_in_normal_duplicate_binding_validation`
- `http_response_pipeline_preserves_the_owned_raw_request_boundary`
- `controlled_http_pipeline_intercepts_raw_request_and_decodes_2xx_json`
- `json_without_status_policy_decodes_a_non_success_response`
- `require_success_rejects_before_json_decoding`
- `json_failure_retains_source_and_complete_response`
- `http_failure_and_cancellation_preserve_distinct_outcome_channels`
- `http_pipeline_transforms_and_maps_at_most_once`
- `controlled_trace_records_raw_success_when_json_mapper_fails`
- `live_and_controlled_http_pipelines_produce_equivalent_component_results`
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

## Active ADR-0009 Standard-Input Scenarios

The cross-profile `stdin_source` suite exercises the public descriptor,
controlled vocabulary, and exact Unix assembly boundary:

- `stdin_descriptor_and_errors_are_typed_fixture_data`
- `controlled_stdin_delivers_lines_and_normal_eof`
- `controlled_stdin_delivers_typed_failure_without_live_fallback`
- `live_stdin_binding_is_exact_and_required`
- `live_stdin_binding_accepts_a_composed_capability`
- `live_stdin_rejects_duplicate_or_competing_process_bindings`

The first three scenarios are platform-neutral. They establish
that `StdinLines` implements
`SourceDescriptor<Item = String, Error = StdinError>`, constructible read/UTF-8
error data, deterministic controlled line/failure/EOF delivery, and no
live-world fallback. The last three are Unix-only and establish missing and
foreign exact binding rejection, direct and composed exact binding, and
rejection of duplicate or competing bindings even when capability identities
differ.

Unix live-runtime unit evidence exercises the terminal reader mechanism with
an injected stream rather than ambient process input:

- `stdin_line_decoder_handles_chunking_crlf_empty_and_unterminated_lines`
- `stdin_reader_delivers_framed_lines_then_eof_from_injected_stream`
- `stdin_reader_stops_at_invalid_utf8_without_synthesizing_eof`
- `stdin_reader_reports_private_mechanism_failure_as_runtime_work_failure`
- `stdin_cutover_is_normal_when_reader_and_owner_run_concurrently`
- `dropping_idle_stdin_reader_interrupts_and_joins_its_thread`
- `stdin_binding_rejects_concurrent_then_allows_sequential_realization`
- `phase6_cancel_preserves_a_synchronous_source_stop_failure`

Together these scenarios prove LF and CRLF removal across chunk boundaries,
empty-line preservation, final unterminated-line ordering before EOF, typed
invalid-UTF-8 termination without a later `Ended`, prompt cancellation and
join while the reader is idle, mechanism-failure separation, synchronous
lease release before reactivation, a normal multi-thread cutover without a
fabricated terminal event, and preservation of an already-completed stop
failure across Cancel. This live evidence and the first-party binding are
Unix-only; no first-party live behavior is claimed for other targets. The
application example separately demonstrates that Component logic stores
`SourceCapability<StdinLines>` and needs no manual stdin thread or mpsc bridge.

## Active ADR-0007 Live Host-Lifecycle Scenarios

The `live_host_lifecycle` suite exercises cancellation-safe owner observation:

- `cancelled_run_forever_observation_preserves_owner_for_explicit_shutdown`

It first polls `run_forever`, lets a fallible host future win an ordinary
`tokio::select!`, verifies ingress remains open, and then exercises both
explicitly selected shutdown modes. Existing Phase 6 and ADR-0006 suites
continue to own detailed fault-race, Drain, Cancel, Port waiter, and zero-work
evidence.

`run_forever_surfaces_dynamic_foreign_capability_before_live_driver` replaces
the old missing-binding fixture, which ADR-0008 correctly moved to synchronous
profile-build rejection. Deliberately non-conforming code hides a foreign
Effect capability until a later Message; `run_forever` then surfaces the
genuine post-build provenance fault, and the bound live Driver is never called.

## Active ADR-0010 Shutdown-Escalation Scenarios

The lifecycle and live-runtime suites cover the accepted contract:

- `shutdown_grace_period_escalates_recurring_timer_and_joins` runs the host
  grace-period composition and checks zero owned work after escalation.
- `shutdown_escalation_joins_pending_effect_without_mapping` and
  `shutdown_escalation_closes_unanswered_request_without_mapping` prove that
  cancelling observation preserves ownership for escalation and joined cleanup,
  without synthesizing application outcomes.
- `shutdown_escalation_before_owner_poll_cannot_restart_work` verifies that
  Drain cannot weaken Cancel or start initial work after that cutoff.
- `shutdown_requests_close_ingress_and_preserve_completed_result` and
  `shutdown_requests_preserve_completed_fault` cover synchronous admission
  closure, repetition, and observation of the original terminal result.
- Internal `shutdown_escalation_preserves_source_fault_completed_before_cancel`
  checks a completed Source fault with Drain both observed and still queued.
- The `RuntimeTask::request_shutdown` rustdoc example compile-checks the public
  host pattern. Existing Drain, Cancel, and fault suites remain regression evidence.

## Active ADR-0006 Live Port-Ingress Scenarios

The following scenarios are contractually accepted. The
`external_port_ingress` suite activated the central identity, routing,
correlation, Drain, Cancel, fault, dropped-waiter, owner-closure, shared-cutoff,
and cloned-notification cases as failing tests before implementation. Public
compile-fail examples enforce the Model-access and inert-Port distinctions. The
broader named set remains the checklist for final conformance audit:

Handle identity and isolation:

- `live_port_handle_validates_exact_program_binding`
- `live_port_handle_rejects_foreign_or_lookalike_port`
- `v2_live_port_handle_cannot_mutate_model`
- `live_port_handle_has_no_controlled_runtime_counterpart`

Notification admission and routing:

- `live_port_notify_uses_bound_protocol_message_path`
- `live_port_notify_success_means_accepted_not_transition_complete`
- `live_port_ingress_shares_component_admission_cutoff`
- `live_port_notify_after_shutdown_or_fault_is_rejected`
- `live_port_notify_cutoff_race_is_owned_or_rejected`

Host Request and correlation:

- `live_port_request_returns_direct_typed_reply`
- `live_port_request_creates_no_request_outcome_or_requester_message`
- `live_port_requests_correlate_reverse_order_replies`
- `v8_live_port_request_preserves_request_reply_causality`

Lifecycle and ownership:

- `live_port_request_drain_retains_until_reply`
- `live_port_request_unanswered_keeps_drain_pending`
- `live_port_request_cancel_wakes_with_runtime_error_without_outcome`
- `live_port_request_clean_closure_wakes_with_runtime_error`
- `live_port_request_fault_preserves_runtime_error`
- `live_port_request_unpolled_future_admits_nothing`
- `v9_dropped_host_waiter_does_not_cancel_request`
- `live_port_request_waiter_is_not_a_second_work_obligation`

These tests assert Protocol routing, causal and terminal behavior, admission
classification, and zero work after successful Cancel. They must not assert an
internal channel, oneshot, correlation-table layout, global event order, or FIFO
among independent handle clones.

## Active ADR-0011 Request-Timeout Scenarios

[`request_timeouts.rs`](../../tests/request_timeouts.rs) covers the contract in
[ADR-0011](../adr/0011-request-timeouts.md):

| Contract | Evidence |
| --- | --- |
| Reply removes deadline; Drain does not wait for it | `controlled_reply_removes_unused_deadline`; `live_reply_cancels_deadline_task_without_delaying_drain` |
| No reply settles once; one obligation; deterministic causal trace | `controlled_timeout_is_one_obligation_with_repeatable_causal_trace` |
| Late reply is harmless; provider work continues | `controlled_late_reply_does_not_repeat_outcome_or_cancel_provider_work`; `live_component_timeout_and_late_reply_drain_cleanly` |
| Zero duration and deadline equality time out | `controlled_zero_and_equal_deadlines_timeout`; `live_zero_and_equal_deadlines_timeout` |
| Occurrence correlation and mapper context | `controlled_concurrent_requests_preserve_explicit_mapper_context`; `live_concurrent_host_timeouts_keep_correlation` |
| Host deadline includes queued time and survives dropped observation | `host_timeout_includes_time_queued_before_owner_starts`; `dropped_host_waiter_keeps_deadline_and_drain_releases_unanswered_request` |
| Scope Cancel preserves abort semantics | `controlled_unbounded_requests_and_scope_cancel_keep_existing_semantics`; `live_scope_cancel_does_not_manufacture_timeout` |
| Overflow and runtime faults remain explicit | `controlled_deadline_overflow_faults_explicitly`; `live_timeout_preserves_runtime_fault_and_rejects_overflow` |

`request_authority_tests` additionally proves both profiles reject foreign
expired reply authority instead of accepting it as a harmless late Reply.

Live timeout tests use paused Tokio time; controlled tests advance logical time.
Neither depends on wall-clock sleeps or incidental task order.

## Remaining Deferrals

- Descriptor/message payload capture, typed trace projections, a public live
  observer, durable trace storage, and replay. In particular,
  `v10_live_observer_has_no_feedback_path` is deferred beyond final v0
  conformance rather than staged for Phase 6.
- Request failure, provider-visible or per-Request cancellation, abandonment,
  and delegation beyond ADR-0011's opt-in timeouts and ADR-0006's external
  whole-scope closure error.
- Notification delivery failure beyond accepted admission and whole-scope
  Cancel/fault cutovers.
- Bounded pressure, overload controls, automatic shutdown deadlines and escalation
  policy, exact shutdown diagnostic counts, per-effect cancellation policy, Driver
  recovery, automatic Source retry, and restartable or shared bridges beyond
  ADR-0004's simple first cut.
- Exact public first-party module/type naming and general live Layer/profile
  binding abstractions where no accepted API already freezes them.
- Named capability bundles, public capability identity inspection, and any
  blessed dynamic or ambient escape hatch. The current Program capability set
  is closed.

No implementation goal may choose a deferred policy merely to make a test
green. The owning phase first updates the governing contract and acceptance
scenario, then receives human approval.

## Final Gate

Phase 7 activates the complete matrix and runs it as the topology-independent
conformance suite. Persistence and durable replay remain outside v0 and have no
scenario in this matrix.
