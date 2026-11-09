# Exploratory Design: Filesystem Capabilities

Status: seed, November 7, 2026; a provisional implementation landed
November 8, 2026 behind the `fs` feature
([ADR-0013](../adr/0013-component-kinds-assemblies-and-first-party-effects.md)):
`AppendToFile`, `ReplaceFile`, `ReadFile`, `RemoveFile`, one `FsError`,
and `FsDriver::new(root)` supplied through `with_drivers` (ADR-0019).
The implementation took positions
on some open questions below without closing them: replace does fsync the
file but not the directory; removing a missing file is `NotFound`, not
success; the root is created lazily by the first write; no ordering is
promised and handles are not modelled. The remaining questions are still the
design work. Raw material came from `examples/pulse`, whose journal now
appends through the first-party effect; see the
[filesystem follow-up](framework-follow-ups.md) for the running summary.

## Governing principle

If a sequence must fail together, it is one effect. The effect boundary is the
atomicity boundary. Write-close-reopen patterns exist for atomicity, not for
the handle, so they become single finite effects with one outcome, and the
Driver performs the multi-step work internally, as the HTTP Driver already
does for a request. There are no `Open` or `Close` effects. "Write only while
open" is runtime state that outcomes delivered as later Messages cannot
type-check; modelling it would turn a race into a different race.

### Effect or Assembly

The atomicity principle pairs with a second question, so any proposed
filesystem thing is classified by two tests:

- **Must these steps fail together?** Then they are one effect. A Component
  that sequences the steps reintroduces the partial-failure window.
- **Does it hold application memory?** State that changes outcomes and cannot
  be recomputed from the current Model (see "Which Memory May Live Outside the
  Model?" in the [API guidance](../api-guidance.md)). Then it is a Component,
  usually an Assembly. Mechanism memory inside a Driver, such as a cached
  handle or an ordering queue, does not count.

| Thing | Fail together? | Application memory? | So |
|---|---|---|---|
| Append a line | Single step | No | Effect |
| Atomic replace (temp, fsync, rename) | Yes | No | One effect |
| Read a file | Single step | No | Effect |
| Remove a file | Single step | No | Effect |
| Watch a file | Not applicable | No, derived from the Model | Source |
| Ordered appends | No | Only if "has it landed" matters | Driver FIFO as mechanism, else Assembly |
| Coalesced saves of `T` | No | Yes, the pending value | Assembly: `Store<T>` |
| Rotate by size or day | The rotation itself, yes | Yes, size and schedule | Assembly issuing one `Rotate` effect |
| Retry on I/O error | No | Yes, the attempt count | Assembly or Layer, never the effect |

The effect set that falls out is small: `Append`, `Replace`, `Read`, `Remove`,
plus a `Watch` source. Everything interesting is an Assembly on top. So the
effects do not have to be designed first: write the Assemblies (`Store<T>`,
a rotating journal) on hand-rolled descriptors as pulse does, and the
descriptors they actually issue are the first-party module. The failure mode
in the other direction is an Assembly that is secretly one effect, such as a
store that writes a temp file in one transition and renames in the next; the
controlled runtime will let you test the partial state as if it were a
feature.

Open questions:

- Does the list above miss an operation that is common and atomic? What is
  deliberately left to applications?
- Does `Replace` promise durability (fsync) or only atomicity of the rename?
  Both, as separate descriptors, or a flag?

## Why the capability model forces names

An ambient filesystem never asks which kind of file use a piece of code is. A
closed capability inventory asks at assembly, and the answer is visible to the
host and to controlled execution. Five uses, not all of them descriptors:

1. **A declared sink or store.** The journal. An inert handle token issued at
   assembly and bound to a path by the host.
2. **A declared working directory.** A Component names files at runtime, but
   within a root the host chose. The capability is the root; descriptors carry
   relative paths.
3. **Arbitrary paths.** A file manager. The capability says so, and a reader of
   the inventory can see it.
4. **A watched file.** A Source, not an effect.
5. **Temporary files inside one operation.** Driver-internal; not a capability.

A sink is a root plus a fixed name, so uses 1 and 2 can share one bound-root
mechanism.

Open questions:

- Is a sink literally a root with a fixed relative name, or a distinct token
  type with a smaller descriptor surface? The first is one mechanism; the
  second reads better at the call site.
- How does use 3 appear in the inventory so that it is obviously broader than
  use 2? A separate capability type, or a root bound to `/`?
- Does the controlled runtime need to know anything about roots, or is a
  relative path in an intent enough for a test to assert?
- Where does use 4 belong: this module, or alongside other Sources?

## Owned and Shared handles

- **Owned:** exactly one Component holds the token, enforced at build the way a
  Port must be bound exactly once. Appends land in that Component's issue
  order and nothing else interleaves. Precedent for exact versus type-wide
  binding: stdin is an exact source boundary.
- **Shared:** several Components hold the token. Each append is atomic and each
  Component's order is kept; there is no cross-Component order, matching the
  existing rule for independent live events.

Open questions:

- Is Owned a property of the token (two token types) or of the binding (one
  token, the builder checks holder count)? The second keeps one descriptor
  family; the first makes the guarantee visible in Component signatures.
- What does an Owned handle mean for an Assembly installed twice?
- Can a Shared handle offer any cross-Component promise, such as "no torn
  lines", and is that the Driver's job?

## Ordering

Ordering is a property of the sink that opts in, never a serialization of all
of a Component's effects; three HTTP checks in pulse must stay concurrent.

Prerequisite (an ADR extending ADR-0002 and ADR-0004): today the live runtime
calls a Driver's `execute` inside the spawned effect task, so two effects
issued back to back may reach the Driver in either order. Calling `execute` on
the dispatch path in issue order and spawning the returned future lets a
Driver that enqueues synchronously preserve per-Component FIFO. The ADR must
also state that controlled `next_effect` order matches issue order.

Open questions:

- Is the per-Component invocation order promised for all Drivers, or only
  stated as what ordered sinks rely on? Promising it for all is simpler to
  explain and costs nothing today.
- What does a Component that needs "this append happened before I say so"
  do? Sequence through outcomes, as the journal does now. Is that the
  documented pattern, or should an ordered sink's outcome mean "durably
  appended in order" so the Component need not wait?
- How does ordering interact with cancellation: if an earlier queued append is
  cancelled by shutdown, do later ones still run under Drain?

## Lifecycle and failure

- Queued appends flush on Drain and drop on Cancel, per ADR-0010.
- I/O errors are `Failed` outcomes with a typed error naming the operation.
- Reopening after an error is Driver mechanism. Whether the Driver retries at
  all is a policy question and therefore belongs in a Layer or the Component.
- The module is feature-gated like the other integrations.

Open questions:

- Does the Driver open at build (fail fast, matches "validate before
  execution") or lazily on first use (no empty files for sinks never written)?
- What is the error type's shape: one enum across operations, or per
  descriptor?
- Does a sink bound to an unwritable path fail `build()` or the first append?

## Sessions are out of scope here

A resource that is genuinely stateful over time (a connection, a file kept
open across writes for a reason) wants a model-derived, reconciled,
generation-carrying resource that both produces events and accepts scoped
effects, with stale work failing as a typed outcome. Sources are the read-only
half of that concept; the Subscription restart revision follow-up and the
read-only `TcpBytes` are the same gap seen from two sides. Name it once rather
than designing it three times. Nothing in this note needs it.

## What pulse should teach before this is designed

- How much sequencing code the journal needs to append in order through
  outcomes alone, which is the cost the ordered sink would remove.
- Whether a controlled test ever wants to see the path, which decides whether
  roots are visible to controlled execution.
- ~~Whether `GetTime` and `AppendLineToFile` want to migrate to the crate at
  the same time.~~ They did, November 8, 2026, as `ReadSystemTime` and
  `AppendToFile`.

## Acceptance evidence to require

- An ADR for the ordering promise with live and controlled tests.
- Build-time tests for Owned handle holder counts and for unbound roots.
- A controlled test that asserts appends by intent, with no path:
  `tests/fs_effects.rs` does this with a relative path only.
- The pulse journal migrated onto the first-party module with no change to its
  controlled tests except the descriptor name: migrated; its controlled tests
  do not exist yet, so this is unproven.
