# ADR-0021: Implicit Identities and Diagnostic Names

- Status: Accepted for implementation.
- Date: November 9, 2026
- Amends: ADR-0003, ADR-0008, ADR-0013, and ADR-0019.

## Decision

Component registration is `component::<C>(config)`. The Program builder issues
an opaque, Program-local `ComponentId` when registration succeeds. Applications
address instances through the returned typed `ComponentRef`, not a constructed
name. Optional `.name("Primary checker")` supplies diagnostic metadata; the
default is the Component kind's Rust type name. Names need not be unique and
do not participate in identity, routing, reconciliation, or semantic trace
equality. Separate installations of one Assembly need no name coordination.
The numeric representation of the identity remains private. Program provenance
continues to be checked on references, connectors, and handles.

`Subscription::source(&capability, descriptor)` and
`Subscription::source_with(&capability, descriptor, mapper)` use a default
Component-local slot identified by the exact Source capability. The default
does not depend on the descriptor, mapper, source location, or list position.
Equal descriptors retain the Source and install the latest mapper; a changed
descriptor replaces it with the existing hard-generation cutover. Omitting
the slot cancels it. Another Component may independently use the same slot.

`.key(value)` overrides the default slot with a Component-local application
key. Keys preserve Rust type and value equality and require owned
`Eq + Hash + Debug + Send + Sync + 'static` values; `Clone` is not required.
For example, `.key(peer.id)` distinguishes subscriptions in a dynamic collection. Explicit keys occupy a
separate namespace from defaults and are independent of the capability, so a
keyed subscription can replace its capability without losing logical identity.
Two equal keys of the same Rust type, or two unkeyed subscriptions through the
same capability, are rejected before reconciliation changes bookkeeping.
Duplicate defaults report that `.key(...)` is needed to distinguish instances.

Controlled Source injection and descriptor inspection accept the capability
for its default slot, or `SubscriptionId::key(value)` for an explicit slot.
`Subscription::id()` remains available for inspection. Default selectors retain
capability provenance so a foreign Program's capability cannot select an input.
Application keys and their equality/hash implementations must be pure and stable.

## Determinism and ordering

Initial controlled work follows successful Component registration order.
Dropping an unfinished registration or rejecting foreign capabilities creates
no Component. Repeated assembly with the same registration sequence produces
the same identities and structural trace regardless of diagnostic names.
Reordering registrations changes the controlled input program and can change
its trace. This explicitly replaces ADR-0003's name-based canonicalization.

Subscription lifecycle changes follow deterministic projection traversal:
desired subscriptions in declaration order, followed by removals in prior
retained order. Reordering a collection does not change its keys or restart
retained Sources. It can change the trace order of independent lifecycle work.
Applications must produce deterministic projections for controlled execution;
unordered collection iteration is not canonicalized by the runtime. No ordering
of independent live events is promised. Causal dependencies remain explicit.

## Invariants and failure modes

- Pure transitions, exclusive Model ownership, typed Commands, controlled
  execution without live Drivers, and per-Component serialization are unchanged.
- Names are diagnostic only, including duplicate or empty names. Internal IDs
  disambiguate diagnostics; applications cannot forge a Component address.
- Explicit key collisions and ambiguous defaults fail before Source lifecycle
  changes. They are never resolved by list position or silently merged.
- Changed descriptors/capabilities retain generation cutover and stale-event
  suppression. Mapper changes alone retain Sources.
- Identity values are scoped to their Program/Component, not durable addresses.
  Persistence and replay remain out of scope.

## Evidence

`tests/identity.rs` covers unnamed and duplicate-named Components, independent
routing, name-independent controlled traces, default subscription retention,
replacement/removal, ambiguous defaults, typed keys, keyed reordering and
capability changes, foreign selectors, and live/controlled default-slot parity.
`tests/typed_program.rs` verifies repeated Assembly installations with identical
diagnostic names. The reconciler's unit tests verify transactional duplicate
rejection for default slots and explicit keys.
Existing kernel, generation-cutover, capability-provenance, Assembly, and runtime
suites remain required. The V4, V6, V7, V11, and V12 acceptance evidence is updated.

## Migration and rollback

Remove the ComponentId argument; use `.name(...)` only when a diagnostic label
adds information. Remove subscription name arguments. Use explicit application
keys for multiple logical instances of one capability or an identity that must
survive a capability change. Test selectors use the capability or an explicit
key. Port naming is unchanged.

Rollback requires restoring explicit Component/subscription names and the
previous ordering contract together with their tests. No persisted data or wire
format changes. A name-only partial rollback must not affect runtime identities.
