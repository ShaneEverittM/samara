# Samara v0 Acceptance Matrix

- Status: Phase 5 contract approved; controlled-execution tranche ready
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
[ADR-0003](../adr/0003-controlled-execution-semantics.md) records the Phase 5
refinements. Exact harness APIs for staged scenarios may be chosen with the
phase that first needs them.

## Vision Traceability

| Vision requirement | Named scenarios | Current contract evidence | Activation |
| --- | --- | --- | --- |
| V1 Repeatable Component Transition | `v1_same_input_produces_equivalent_model_and_command_intent`; `v1_equivalent_effect_outcomes_produce_equivalent_messages`; `v1_update_has_no_runtime_capability` | Active direct-transition and typed-intent tests; the trait signature is a compile contract. Stored mapper execution is staged. | Phase 3, completed in Phase 5 |
| V2 Isolated State Ownership | `v2_same_component_transitions_never_overlap`; `v2_live_handle_cannot_mutate_model`; `v2_cross_component_interaction_reenters_as_message`; `v2_protocol_message_remains_distinct_from_provider_message` | Component/handle type separation and Protocol conversion compile now. Serialization and delivery are staged. | Phase 3, repeated under live concurrency in Phase 6 |
| V3 Interceptable Effect Descriptor | `v3_effect_command_exposes_typed_descriptor`; `v3_opaque_async_work_is_not_an_effect_descriptor`; `v3_controlled_effect_maps_exactly_once_without_live_driver` | Typed descriptor inspection and compile-fail API constraints are active. Runtime interpretation is staged. | Phase 4, completed in Phase 5 |
| V4 Declarative Subscription Lifecycle | `v4_subscriptions_are_pure_model_projection`; `v4_new_identity_starts_source`; `v4_equal_descriptor_retains_source_and_adopts_latest_mapper`; `v4_removed_identity_cancels_source`; `v4_changed_descriptor_replaces_source`; `v4_replaced_generation_discards_stale_work`; `v4_composed_source_plan_reaches_terminal_controlled_behavior`; `v4_controlled_events_use_reusable_mapper`; `v4_source_failure_enters_as_message` | Projection, separate identity, descriptor comparison, and Layer composition are active. ADR-0003 settles the retained-mapper, hard-cutover, and SourcePlan behavior for runtime activation. | Phase 4 reconciliation, completed in Phase 5 and repeated live in Phase 6 |
| V5 Live and Controlled Program Parity | `v5_one_program_factory_compiles_for_both_profiles`; `v5_reference_components_are_profile_invariant`; `v5_live_and_controlled_boundaries_map_equivalent_observations` | The reference examples compile one program factory into both assembly shapes. Runtime parity is staged. | Phase 6 |
| V6 Program-Wide Controlled Determinism | `v6_identical_controlled_runs_have_equal_structural_trace_and_final_state`; `v6_equal_final_state_with_different_structural_trace_is_not_equivalent`; `v6_typed_descriptor_payload_is_compared_by_direct_intent_test` | ADR-0003 defines structural trace equivalence and keeps typed domain-payload comparison in direct intent tests rather than requiring generic trace payload capture. | Phase 5 |
| V7 Controlled Time Progression | `v7_manual_advance_uses_no_wall_sleep`; `v7_automatic_advance_reaches_next_instant`; `v7_equal_time_uses_deterministic_causal_insertion_order`; `v7_equivalent_schedules_are_repeatable`; `v7_initial_work_uses_component_id_not_registration_order` | ADR-0003 fixes the v0 scheduler key as logical deadline plus deterministic insertion ticket. Assembly and drive signatures compile; no placeholder call counts as behavioral evidence. | Phase 5 |
| V8 Causal Semantics Without Global Order | `v8_causal_edges_are_preserved`; `v8_component_transitions_do_not_overlap`; `v8_independent_live_events_accept_either_order`; `v8_conformance_compares_partial_order_not_scheduler_sequence` | The assertions are specified without mailbox, queue, task, or global-order vocabulary. | Phase 3 serialization, Phase 5 controlled causality, Phase 6 live independence, final audit Phase 7 |
| V9 Structured Work Ownership | `v9_drive_reports_semantic_pending_work`; `v9_pending_now_counts_ready_messages_and_due_timers`; `v9_pending_later_counts_effects_future_timers_sources_and_requests`; `v9_controlled_cancel_leaves_zero_work`; `v9_live_shutdown_leaves_zero_work`; `v9_effect_cancellation_has_one_outcome`; `v9_source_cancellation_emits_no_unpromised_event` | ADR-0003 defines the controlled accounting units. Ownership/report types compile, but fabricated reports are not evidence. Public observation points are defined below. | Phase 5 controlled; Phase 6 live; final audit Phase 7 |
| V10 Non-Influential Semantic Trace | `v10_reading_controlled_trace_after_drive_has_no_feedback_path`; `v10_trace_explains_transitions_work_time_and_causation`; `v10_trace_records_stale_source_drops`; `v10_live_observer_has_no_feedback_path` | ADR-0003 requires always-collected, read-afterward structural records with logical time, parentless roots, and exactly one immediate parent for every non-root. Streaming observer APIs are not required in Phase 5. | Phase 5 controlled, Phase 6 live |
| V11 Shallow Tokio Onboarding | `v11_minimal_tokio_mpsc_application_compiles`; `v11_minimal_component_runs_with_controlled_stream`; `v11_minimal_component_runs_with_live_mpsc` | The minimal example and direct Component test are active. Its controlled runtime test remains visibly staged rather than pretending the façade runs. | Phase 5 controlled; completed in Phase 6 |

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

The demanding controlled example functions remain compile-checked staged
scenarios. They become active tests only when their runtime slice exists.

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

## Remaining Deferrals

- Descriptor/message payload capture, typed trace projections, streaming
  observers, durable trace storage, and replay.
- Request failure, timeout, abandonment, late-Reply, delegation, and in-band
  cancellation semantics.
- Notification delivery failure and shutdown policy cases listed in
  `docs/api-contract.md`.
- `bytes` adoption and Decoder EOF/finalization, which remain required before
  live TCP framing in Phase 6.

No implementation goal may choose a deferred policy merely to make a test
green. The owning phase first updates the governing contract and acceptance
scenario, then receives human approval.

## Final Gate

Phase 7 activates the complete matrix and runs it as the topology-independent
conformance suite. Persistence and durable replay remain outside v0 and have no
scenario in this matrix.
