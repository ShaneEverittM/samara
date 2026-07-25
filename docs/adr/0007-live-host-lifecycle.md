# ADR 0007: Live Host Lifecycle

- Status: Accepted
- Date: July 25, 2026
- Decision owners: Samara maintainers
- Extends: [ADR 0004](0004-initial-live-runtime-semantics.md)

## Context

`RuntimeTask::shutdown` owns structured closure, but it is not an observation
API. A long-running host currently has to delay for an arbitrary duration before
calling it. A Driver or runtime fault that closes the owner during that delay is
therefore not surfaced until shutdown is eventually attempted.

Hosts also need to compose runtime completion with process-specific shutdown
conditions such as Ctrl-C, a service supervisor, or a test future. Samara must
not hard-code one signal, hide an error from the host's shutdown future, or
silently choose between ADR-0004's materially different Drain and Cancel
semantics.

## Decision

### `RuntimeTask::run_forever` observes the live owner

The initial host-lifecycle API is:

```rust,ignore
impl RuntimeTask {
    pub async fn run_forever(
        &mut self,
    ) -> Result<ShutdownReport, RuntimeError>;
}
```

`run_forever` waits for the runtime owner to terminate and returns that owner's
terminal result after its structured cleanup. It does not initiate shutdown,
close ingress, or select a shutdown policy. A healthy program intended to run
indefinitely therefore leaves this future pending; a runtime fault completes it
as soon as the owner has cancelled and joined or aborted all runtime-owned
work.

The method lives on `RuntimeTask`, rather than `LiveRuntime`, because
`LiveRuntime::spawn` is the point at which the host receives the unique
structured owner. Existing Component and Port handles remain obtained before
spawn and retained beside that owner.

### The observation future is cancellation safe with respect to ownership

`run_forever` borrows the `RuntimeTask` mutably instead of consuming it. If a
host `select!` branch cancels the observation future, the `RuntimeTask` still
owns the runtime owner task. Cancelling observation alone does not close
ingress, abort work, detach a task, or manufacture an application outcome.

The host may then consume the same task through
`RuntimeTask::shutdown(Shutdown::Drain)` or
`RuntimeTask::shutdown(Shutdown::Cancel)`. Dropping the task continues to use
ADR-0004's existing structured Cancel fallback.

If runtime completion and a host shutdown condition become ready together,
Tokio may select either independent host branch. This adds no ordering promise.
A runtime fault that has won its terminal race remains preserved: it is returned
either by `run_forever` or by the subsequent `shutdown` boundary and is not
replaced by clean host cancellation.

### Signal and shutdown policy remain host concerns

Samara supplies no Ctrl-C-specific method and does not accept or discard the
output of a host shutdown future. Ordinary Tokio composition keeps both signal
success and signal failure visible to the host:

```rust,ignore
let mut runtime = runtime.spawn();

tokio::select! {
    result = runtime.run_forever() => return result,
    signal = tokio::signal::ctrl_c() => signal?,
}

runtime.shutdown(Shutdown::Drain).await
```

The final call names Drain or Cancel at the decision point. A host may apply its
own deadlines, escalation, multiple signals, or supervisor protocol around this
primitive without changing Samara runtime semantics.

## Consequences

### Positive

- Runtime faults no longer need an arbitrary host sleep or a later shutdown
  attempt before they become visible.
- `tokio::select!` composes runtime termination with any host-owned future while
  preserving errors from both boundaries.
- Cancelling observation does not cancel or detach the runtime.
- Drain and Cancel remain explicit and retain their accepted ADR-0004 meanings.
- Samara does not acquire process-global OS signal policy.

### Negative

- Hosts write one ordinary `tokio::select!` when they have a shutdown signal;
  Samara does not collapse signal installation, error mapping, and policy into
  one convenience call.
- The name describes the normal healthy duration even though faults and future
  clean terminal paths can make the method return.
- `RuntimeTask` remains mutably borrowed while its terminal result is being
  observed, so one task cannot concurrently call another ownership method on
  it without first ending that observation.

## Failure Modes

- A runtime or Driver fault completes `run_forever` with the preserved
  `RuntimeError` only after structured fault cleanup.
- Cancelling the observation future leaves the `RuntimeTask` responsible for
  the still-running owner.
- Dropping that `RuntimeTask` invokes the existing Cancel fallback and may
  discard work that Drain would retain.
- A host shutdown future that fails remains a host error. If the host returns
  early, dropping `RuntimeTask` still closes ownership through Cancel.
- Drain can still wait forever; this API introduces no deadline or automatic
  Drain-to-Cancel escalation.

## Required Evidence

- A dynamically reached runtime fault completes `run_forever` without the host
  first calling `shutdown`.
- Cancelling a pending `run_forever` branch leaves ingress usable and permits a
  subsequent explicit clean shutdown.
- The documented Tokio selection pattern compiles with a fallible host shutdown
  future and names Drain or Cancel outside `run_forever`.
- Existing Drain, Cancel, fault cleanup, ingress cutoff, and zero-owned-work
  conformance tests remain unchanged.

## Explicitly Unresolved

- a shorter `wait` alias or a consuming convenience that owns a shutdown
  future;
- first-party Ctrl-C, Unix signal, service-supervisor, or readiness policy;
- deadlines, grace periods, escalation, and automatic Drain-to-Cancel behavior;
- richer clean-completion and owner-task failure taxonomy; and
- public live tracing or observation beyond this one terminal lifecycle result.

## Rollback

`RuntimeTask::run_forever` can be removed without changing runtime execution,
shutdown, ingress, Driver, or Component semantics; hosts can await the same
owner through a future replacement API. Changing observation cancellation to
initiate shutdown, selecting a default shutdown mode, hiding host-future
results, or weakening fault preservation requires a superseding ADR and
matching evidence.
