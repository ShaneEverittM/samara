# Samara v0 Milestone API Contract

- Status: Phase 4 accepted; Phase 5 controlled-execution contract approved
- Date: July 23, 2026
- Scope: Change-controlled public API slices for staged implementation

## Purpose

This document turns the compiler-checked consumer sketch into an implementation
contract without pretending that every v0 policy has already been selected.
The public declarations in `src/lib.rs`, the reference Components in
`examples/`, and the scenarios in `docs/testing/v0-acceptance-matrix.md` form
the executable side of this contract.

An API slice becomes frozen when Shane accepts the phase audit that activates
it. A later Goal-mode run must not silently change a frozen slice. If the
implementation demonstrates that a frozen shape is contradictory or
impractical, the run stops and presents the evidence for a separate contract
revision.

"Frozen" here means change-controlled for Samara's implementation program. It
does not claim ecosystem stability or prevent an explicit, reviewed revision
before a public release.

## Phase 2 Cutover Rule

The Actor-based proof of concept is historical evidence, not a compatibility
target. Its mailbox errors, singleton-per-type registration, direct ask/tell
futures, erased envelopes, and opaque future-based effects do not constrain the
new runtime.

The root `samara` crate now owns the candidate API. The former disposable
`design/api-sketch` crate must not remain as a second source of truth. Reference
Components compile against the root crate, and new acceptance tests begin from
those Component contracts rather than adapting old PoC fixtures.

Implementation techniques from the PoC may be reused only when they satisfy the
new acceptance scenarios and do not become observable topology.

## Frozen for Phase 3: Component Kernel

The accepted Phase 2 audit constrains Phase 3 to this slice:

- `ComponentId` is stable logical identity, not a task, mailbox, type singleton,
  or registration-order token.
- A `Component` value is immutable logical configuration. Behaviorally relevant
  mutable state belongs in `Component::Model`.
- `Component::Message` is the only transition input.
- `Component::init(&self) -> Init<Model, Message>` declares initial state and
  optional startup work.
- `Component::update(&self, &mut Model, Message) -> Command<Message>` is the v0
  Rust spelling of the conceptual pure transition. Exclusively owned in-place
  mutation is observationally equivalent to returning a new Model.
- One composable `Command<Message>` is returned. `Command::none` represents no
  finite work and `Command::batch` groups declarations; batching does not
  promise completion order.
- `Component::subscriptions(&self, &Model) -> Subscriptions<Message>` is a pure
  projection with an empty default.
- The base Component contract requires movable configuration, Model, and
  Messages, but does not require the Component configuration itself to be
  `Sync`. Per-Component serialization does not require concurrent access to the
  configuration, and a stronger bound would prematurely constrain placement.
- `ComponentRef<C>` is an inert typed logical address. It grants neither model
  access nor live delivery capability.
- `ComponentHandle<C>` is a live boundary capability and is deliberately
  distinct from `ComponentRef<C>`.

This slice freezes observable ownership and transition shapes, not the internal
container, task, queue, lock, or scheduling topology used to implement them.

## Compile-Checked Candidate Slices

The following public shapes remain in the root crate so the three reference
applications continue to exercise the intended end state. They are
change-controlled design candidates, but their implementation phases must
activate and audit their acceptance tranche before freezing them:

- Phase 6: live Drivers, first-party Tokio bridges, live ingress, runtime scope,
  and structured shutdown.

The reference examples are normative about the application shape they show.
Their placeholder runtime calls are not evidence that the corresponding
runtime policy is frozen.

## Frozen for Phase 4: Declarative Work Kernel

The accepted Phase 4 audit freezes the declarative finite-work and ongoing-work
boundaries implemented by `Command`, `EffectInvocation`, `Subscription`,
Subscription reconciliation, and the first `Framed` Source Layer. In
particular:

- EffectDescriptors remain separately interceptable from their pure one-shot
  message mappers and need not be `Clone` or comparable.
- Each Command occurrence is distinct even when descriptor values look equal.
- Subscription reconciliation compares Component-local identity and typed
  SourceDescriptor equality, never mapper object identity.
- A reusable SourceEvent mapper and deterministic Layer composition remain
  inert until a runtime profile supplies terminal behavior.
- `Framed` Layers are profile-independent and may carry deterministic
  runtime-scoped decoder state without performing ambient I/O.

The exact Decoder EOF/finalization contract remains provisional with the
planned `bytes` migration and must be resolved before live TCP framing in Phase
6, as recorded by the Phase 4 audit.

## Approved for Phase 5: Controlled Execution

[ADR-0003](adr/0003-controlled-execution-semantics.md) governs the observable
controlled-runtime behavior that Phase 5 may now implement:

- A retained Source realization atomically adopts the latest post-transition
  Subscription mapper.
- Replacing a Source is a hard private-generation cutover; stale work that has
  not begun a transition is dropped and traced.
- Composed SourceDescriptors automatically lower to a runtime-owned
  `SourcePlan`; applications bind controlled behavior only for terminal
  descriptors.
- Equal-time controlled work follows logical deadline and deterministic causal
  insertion order.
- Controlled execution always records an in-memory structural trace with
  logical time, parentless roots, and exactly one immediate causal parent for
  every non-root record.
- `ProgramBuilder::build()` is fallible for explicitly knowable assembly
  errors, without claiming a closed static dependency graph.
- Missing controlled terminal behavior faults the run at that boundary and
  never falls through to a live Driver.
- Work reports count semantic obligations as `pending_now` and
  `pending_later`, not tasks, queues, or other runtime mechanics.
- Phase 5 implements only the successful typed Request/Reply lifecycle through
  `RequestOutcome::Replied`.

The ADR freezes these observable semantics, not the runtime's container,
scheduler, type-erasure, or storage implementation. Exact Rust spellings may
be selected during Phase 5 where the accepted contract does not already name
them, then reviewed at the phase audit.

## Deliberately Unfrozen Surfaces

The following decisions remain explicit gates or deferrals rather than
accidental promises made by a placeholder type or variant:

- Request failure, deadline, cancellation, late-Reply, abandoned-Reply, and
  delegation policies beyond Phase 5's successful `Replied` path.
- Notification delivery-failure semantics.
- The complete public Command/conformance inspection API, including sends,
  timers, batches, and stored message mappers.
- The exact `ProgramBuilder::build()` error taxonomy beyond ADR-0003's
  validation scope.
- The general Rust shape of Layers, SourcePlan lowering, and live/controlled
  profile bindings beyond ADR-0003's application-facing behavior.
- Driver cancellation details and the meaning of a SourceDriver returning
  without explicitly ending or failing its Source.
- Descriptor/message payload tracing, typed trace projections, streaming and
  live observer APIs, and any durable trace representation.
- Runtime error taxonomy outside the missing-controlled-behavior contract,
  plus backpressure and overload behavior.
- Shutdown drain-versus-cancel policy and final work-accounting units.
- Decoder EOF/finalization semantics and `bytes` adoption before live TCP
  framing in Phase 6.

Before an implementation phase reaches one of these surfaces, its acceptance
contract must either settle the question or explicitly keep the behavior out of
that phase. Stub methods and illustrative enums may change at that gate after
human review.

## Testing Surface

Public application APIs should directly support ordinary Component tests:
construct configuration and Model, deliver a Message, and inspect the next
Model, Command intent, and desired Subscriptions without a runtime.

Broader runtime conformance may use a first-party harness rather than expanding
every internal detail into the application API. Harness observations must be
semantic and topology-neutral. In particular, tests compare typed descriptor
intent, mapped Messages, causal relationships, logical time, and final state;
they do not compare closure identity, queue position, task identity, or one
chosen order for independent events.

The current Command inspection methods are sufficient for the accepted Phase 4
reference tests. Inspection of sends, timers, batches, and stored mappers
remains change-controlled future work; the accepted slice does not claim a
complete public L1 inspection API.

## Phase 2 Exit Criteria

Phase 2 is ready for human acceptance when:

1. The root crate contains the candidate façade and the old PoC is absent from
   the active build.
2. All three reference Components compile against `samara`.
3. The executable Phase 2 tests and doctests pass, including the `ReplyTo`
   `#[must_use]` compile contract.
4. Every V1-V11 requirement maps to named scenarios and a planned phase.
5. Missing semantics and non-executable promises are listed rather than hidden
   behind passing placeholder tests.
6. Formatting, documentation, Clippy, and stale-vocabulary checks pass.
