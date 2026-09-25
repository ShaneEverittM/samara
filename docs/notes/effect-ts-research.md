# Lessons from TypeScript Effect

Research notes, September 25, 2026. Exploratory, not accepted API decisions.
Core references below use Effect's v3 documentation; specialized Workflow APIs
are separate from ordinary Effects. Preserve Samara's pure Model transitions,
explicit descriptors, controlled execution, and Layer/Driver boundary.

- **Composable schedules are the smallest useful experiment.** Separate the
  decision to retry or repeat from executing the operation: selected failures,
  finite attempts, exponential delays, fixed intervals versus delays after
  completion, and explicit overrun behavior. A downstream pure helper can take
  an outcome and Model-owned schedule state and return `Stop` or a next state
  and delay. Components use `Command::after`; jitter needs explicit controlled
  inputs. A fluent Effect retry Layer would need extension points beyond the
  [currently private continuation mechanism](../adr/0012-http-redirect-layer.md).
  See [schedules](https://effect.website/docs/v3/scheduling/introduction) and
  [built-in policies](https://effect.website/docs/v3/scheduling/built-in-schedules).

- **Task ownership does not guarantee asynchronous resource release.** Effect
  scopes pair acquisition with finalization. Samara's cancellation currently
  drops Driver futures and joins aborted tasks; this cannot guarantee an async
  lease release or GPIO safe-state write. Investigate a narrow Driver cleanup
  contract when a concrete integration needs one: acquisition/cancellation
  races, cleanup completion, preserving original and cleanup failures, and
  eventual hard cancellation. Application compensation remains Component
  policy. Unbounded finalization could make Cancel wait indefinitely.
  See [Effect scopes](https://effect.website/docs/v3/resource-management/scope),
  [Driver contract](../../src/lib.rs), and
  [current shutdown semantics](../adr/0004-initial-live-runtime-semantics.md).

- **Distinguish admission, queued work, and active concurrency.** Limiting active
  Drivers alone leaves an unbounded backlog. Specify producer suspension,
  rejection, coalescing, and shedding where appropriate, with pressure reaching
  Sources. Blocking the runtime owner on capacity can prevent it from processing
  the completion that frees capacity. A worker Component can explore policy,
  but cannot establish end-to-end bounds over today's unbounded delivery.
  Effect's `all` defaults to sequential execution; Samara's `Command::batch`
  describes independent work. Preserve that distinction.
  See [queues](https://effect.website/docs/v3/concurrency/queue),
  [concurrency](https://effect.website/docs/v3/concurrency/basic-concurrency), and
  [Samara's delivery contract](../api-contract.md#accepted-for-phase-6-initial-live-runtime).

- **Learn from dependency assembly and resource sharing.** Effect's Layers
  construct services and specify shared versus fresh instances. That is useful
  precedent for composable Programs and shared Driver resources. Samara already
  declares capabilities and validates bindings. Its Layer means transformation
  of declared work, so do not import Effect's terminology or ambient service
  access into `update`. See
  [Layers](https://effect.website/docs/v3/requirements-management/layers) and
  [memoization](https://effect.website/docs/v3/requirements-management/layer-memoization).

- **Preserve existing failure and test boundaries.** Samara already separates
  typed Effect failures, cancellation, and runtime faults, and controls both
  outcomes and logical time. Effect's failure composition may become useful
  when one operation and its cleanup both fail; a general cause tree is not yet
  justified. Borrow concise time-based test recipes rather than another test
  service architecture. See [Cause](https://effect.website/docs/v3/data-types/cause)
  and [TestClock](https://effect.website/docs/v3/testing/testclock).

## Identity correction

Required `idempotencyKey` belongs to specialized `Workflow.make`, not generic
Effect. It contributes to workflow execution identity; it is not request
correlation, freshness, sequencing, or a guarantee of exactly-once external
writes. Samara should require identity only for a defined guarantee. Durable
workflows remain outside v0. See
[Workflow source](https://github.com/Effect-TS/effect/blob/v3/packages/workflow/src/Workflow.ts)
and [generic Effect source](https://github.com/Effect-TS/effect/blob/v3/packages/effect/src/Effect.ts).

## First experiment

Add an example-local retry schedule to `urlchecker`: one HTTP attempt at a time,
retry selected transient outcomes after 100/200/400 ms, and retain Model-owned
selection revisions to reject stale outcomes and retry timers. Test attempts,
exact logical deadlines, success, permanent failure, and selection changes.
Use current public APIs first to discover whether reusable policy is sufficient
or a public Layer continuation API is needed.
