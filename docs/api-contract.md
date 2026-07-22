# Samara v0 Milestone API Contract

- Status: Phase 2 accepted; Component-kernel slice frozen
- Date: July 22, 2026
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

- Phase 4: `Command`, EffectDescriptor and EffectOutcome composition,
  `Subscription`, SourceDescriptor and SourceEvent composition, Layers,
  Protocols, Ports, Requests, Replies, and Program assembly.
- Phase 5: controlled bindings, effect completion, Source maintenance, logical
  time, pending-work accounting, state inspection, and semantic trace.
- Phase 6: live Drivers, first-party Tokio bridges, live ingress, runtime scope,
  and structured shutdown.

The reference examples are normative about the application shape they show.
Their placeholder runtime calls are not evidence that the corresponding
runtime policy is frozen.

## Deliberately Unfrozen Surfaces

The following decisions remain explicit gates rather than accidental promises
made by a placeholder type or variant:

- Exact RequestOutcome variants and request deadline, cancellation, late-reply,
  abandoned-reply, and delegation policies.
- Notification delivery-failure semantics.
- Whether a newly declared Subscription mapper replaces the retained mapper
  when identity and SourceDescriptor remain equal, and how stale events from a
  replaced Source are handled.
- The complete public Command/conformance inspection API, including sends,
  timers, batches, and stored message mappers.
- Program graph validation errors and whether `ProgramBuilder::build` is the
  fallible validation boundary.
- The Rust shape of general Layers and live/controlled profile bindings.
- Driver cancellation details and the meaning of a SourceDriver returning
  without explicitly ending or failing its Source.
- The semantic trace representation, observer attachment API, causal IDs,
  logical timestamps, and versioning.
- Runtime error taxonomy, backpressure and overload behavior.
- Shutdown drain-versus-cancel policy and final work-accounting units.

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

The current Command inspection methods are sufficient for the reference
Component tests they exercise. Phase 4 must finish the inspection story before
claiming complete L1 command-emission coverage.

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
