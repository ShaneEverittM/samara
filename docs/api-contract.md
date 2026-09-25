# Samara v0 Milestone API Contract

- Status: Phase 6 accepted; ADR-0005 through ADR-0012 contracts accepted and
  implemented
- Date: July 25, 2026
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

No implementation-phase candidate slice remains open. Shane accepted the Phase 6
audit on September 22, 2026, freezing live Drivers, first-party Tokio bridges,
live ingress, runtime scope, and structured shutdown as described under
[Accepted for Phase 6](#accepted-for-phase-6-initial-live-runtime). The
reference examples remain normative about the application shape they show, and
their live and controlled tests remain regression evidence.

## Frozen for Phase 4: Declarative Work Kernel

The accepted Phase 4 audit freezes the declarative finite-work and ongoing-work
boundaries implemented by `Command`, `EffectInvocation`, `Subscription`,
Subscription reconciliation, and the first `Framed` Source Layer. In
particular:

- EffectDescriptors remain separately interceptable from their pure one-shot
  message mappers and need not be `Clone` or comparable.
- `Command::effect_discarding_outcome` removes only the application Message
  continuation. The effect remains a runtime-owned, controllable, traceable
  finite obligation subject to ordinary Drain, Cancel, and fault behavior.
- Each Command occurrence is distinct even when descriptor values look equal.
- Subscription reconciliation compares Component-local identity and typed
  SourceDescriptor equality, never mapper object identity.
- A reusable SourceEvent mapper and deterministic Layer composition remain
  inert until a runtime profile supplies terminal behavior.
- `Framed` Layers are profile-independent and may carry deterministic
  runtime-scoped decoder state without performing ambient I/O.

The July 25 API revision adopts one consistent continuation convention without
changing these lifecycle semantics or the concrete
`Component::update(...) -> Command<Message>` boundary:

- `Command::effect(&capability, effect)`, `Command::request(port, request)`,
  `Subscription::source(&capability, id, descriptor)`, and
  `HttpResponsePipeline::into_command(&capability)` use the standard
  `Message: From<BoundaryValue>` conversion as their default mapper.
- Their `effect_with`, `request_with`, `source_with`, and
  `into_command_with` counterparts accept an explicit pure mapper.
- The default is appropriate only when one boundary type has one canonical
  Message meaning. Captured domain correlation or call-site-specific meaning
  remains explicit through the `_with` form.
- No runtime lookup, reflection, implicit return conversion, or new Command
  kind is introduced. Both forms lower to the same stored mapper and therefore
  preserve controlled/live equivalence, tracing, and at-most-once or reusable
  mapper semantics as appropriate.

The accepted Phase 4 implementation had no Decoder EOF/finalization hook.
[ADR-0004](adr/0004-initial-live-runtime-semantics.md) resolved that Phase 6
contract gate by adopting `bytes::Bytes` for first-party TCP chunks and
requiring explicit pure Decoder finalization before a Framed Source reports
normal ending. The Phase 6 implementation now spells that operation
`Decoder::finish` and exercises the profile-independent Layer behavior directly;
the same Layer is retained by both runtime profiles. The accepted Phase 6
audit freezes that spelling.

## Accepted for Phase 5: Controlled Execution

[ADR-0003](adr/0003-controlled-execution-semantics.md) governs the observable
controlled-runtime behavior implemented and accepted in Phase 5:

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
- `ProgramBuilder::build()` is fallible for explicitly knowable logical
  assembly errors. ADR-0008 subsequently closes the world-boundary capability
  inventory and requires complete profile binding validation before execution.
- A deliberately hidden foreign capability still faults before controlled
  behavior when first observed; controlled execution never falls through to a
  live Driver.
- Work reports count semantic obligations as `pending_now` and
  `pending_later`, not tasks, queues, or other runtime mechanics.
- Phase 5 implements only the successful typed Request/Reply lifecycle through
  `RequestOutcome::Replied`.

The ADR freezes these observable semantics, not the runtime's container,
scheduler, type-erasure, or storage implementation. Exact Rust spellings that
the accepted contract did not name were selected during Phase 5 and reviewed
at its audit.

## Accepted for Phase 6: Initial Live Runtime

[ADR-0004](adr/0004-initial-live-runtime-semantics.md) governs the accepted,
bounded live-runtime slice, implemented and accepted with the Phase 6 audit.

The ADR specifies:

- successful `ComponentHandle::send` and `SourceSink` operations mean accepted
  for runtime-managed delivery, while closed or faulted runtimes reject new
  work explicitly;
- runtime-internal delivery in a healthy running scope is unbounded and does
  not intentionally drop accepted work because capacity was reached;
  configurable capacity and mature overload behavior are not v0 promises, and
  documented Cancel/fault cutovers may discard queued application work;
- Drain closes ingress, stops Sources, realizes no new Sources, and recursively
  processes accepted and causally emitted finite work, even when that means it
  waits indefinitely;
- Cancel closes ingress, stops application driving, cancels semantic
  obligations and Driver tasks, and manufactures no application outcome or
  event solely because the runtime scope ended;
- successful shutdown closes every runtime-owned task and reports zero
  `remaining`, `pending_now`, and `pending_later`; completed and cancelled refer
  to semantic obligations rather than runtime mechanics, while their exact
  diagnostic counts remain non-normative;
- normal EffectDriver success and failure complete each effect exactly once:
  mapped effects invoke their mapper once, discarded-outcome effects schedule
  no Message, and whole-scope abort produces no outcome; the first accepted
  Source terminal call or active
  SourceDriver return terminates a live generation exactly once while
  cancellation, removal, replacement, shutdown, and runtime fault winning first
  emit no unpromised SourceEvent;
- Driver panic and other live mechanism violations fault the runtime, close
  ingress, initiate structured cancellation, and surface `RuntimeError` rather
  than fabricated typed application data;
- first-party TCP is a single connection per Source realization, emits
  `bytes::Bytes`, and contains no retry, reconnect, or framing policy;
- Decoder finalization runs exactly once on normal underlying EOF, emitting
  final frames before `Ended` or one typed decode failure without `Ended`;
- the first-party `mpsc` receiver binding is a one-shot, single-consumer live
  resource whose closure ends normally and whose duplicate or later
  reactivation faults the runtime explicitly; and
- Phase 6 ships no public live trace or event observer API and tests independent
  live completions by causal partial order rather than controlled trace-vector
  order. ADR-0007's terminal owner observation is a lifecycle boundary, not a
  stream of application or runtime events.

The ADR freezes observable first-cut behavior, not task topology, queue
representation, exact public module naming, or a mature product policy for
overload, shutdown deadlines, Driver recovery, or bridge restartability.

## Accepted and Implemented Contract: Live Port Ingress

[ADR-0006](adr/0006-live-port-ingress.md) extends the existing live ingress
boundary to provider-neutral Ports:

- `LiveRuntime::port_handle(&Port<P>)` returns a cloneable `PortHandle<P>` only
  when the Port belongs to that runtime's exact Program and has its exact built
  Protocol-and-name binding. Foreign or absent exact bindings fail synchronously
  with `RuntimeError`.
- `Port<P>` remains inert logical wiring suitable for Component configuration.
  `PortHandle<P>` is a live capability for surrounding Tokio code and exposes no
  provider, Model, transition, channel, or runtime topology.
- `PortHandle::notify` and `PortHandle::request` attempt admission when their
  futures are first polled. They share the same atomic cutoff as
  `ComponentHandle` and the owning `RuntimeTask`.
- Successful notify means accepted for runtime-managed conversion and delivery,
  not that the provider transition completed.
- An admitted host Request uses the same Port binding, `RequestInvocation`,
  opaque correlation, and `ReplyTo` path as `Command::request`, but its awaited
  success is `R::Reply` directly. It creates neither a requester Component
  Message nor `RequestOutcome::Replied`.
- Drain closes new Port ingress and retains admitted Notifications, Requests,
  Replies, and their causal finite work. An unanswered host Request may keep
  Drain pending forever.
- Cancel or non-fault scope closure before Reply wakes the external waiter with
  `RuntimeError` without manufacturing a RequestOutcome. A runtime fault wakes
  it with the preserved scope fault and rejects later Port ingress with that
  same error.
- Dropping an unpolled Request future admits nothing. Dropping or timing out an
  already admitted waiter does not cancel provider delivery, release the
  outstanding Request, or relax Drain; it relinquishes only host observation.
- `PortHandle` is live-only. ControlledRuntime gains no corresponding handle;
  deterministic tests continue to use the existing controlled drive and
  inspection surfaces.

ADR-0006 freezes the original unbounded host-boundary completion. ADR-0011
below extends it with opt-in timeouts; `Failed`, `Cancelled`, and abandonment
remain deferred.

## Accepted and Implemented Contract: Request Timeouts

[ADR-0011](adr/0011-request-timeouts.md) adds opt-in
`Command::request_timeout`, `Command::request_timeout_with`, and
`PortHandle::request_timeout`. Existing request forms remain unbounded.

- Component durations start at Command interpretation; host durations start at
  admission on first poll, including queued time. Controlled deadlines use
  logical time; live deadlines use Tokio time. Overflow fails explicitly.
- Only a Reply interpreted strictly before the deadline succeeds. At or after
  it, including zero duration, `TimedOut` wins. Each request settles once.
- Component outcomes pass through the normal Message mapper; timed host calls
  return `Result<RequestOutcome<R::Reply>, RuntimeError>`.
- Settlement removes both the request obligation and its deadline bookkeeping.
  Expired reply authority is recognized without a runtime tombstone; its late
  Reply is discarded. Foreign or invalid authority still faults.
- Admitted provider Messages and their work remain owned. Timeout does not
  imply provider cancellation or prevent its external actions.
- Drain processes deadlines and their continuations. Cancel/fault clears them
  under the existing scope policy; host failures remain `RuntimeError`.
- Controlled traces distinguish reply, timeout, and late-Reply drop. Timeout's
  causal parent is its Request, and its deadline adds no separate obligation.

Failure, abandonment, explicit request cancellation, and default timeout
policies remain deferred. Invariants, risks, and rollback are in ADR-0011;
[acceptance evidence](testing/v0-acceptance-matrix.md#active-adr-0011-request-timeout-scenarios)
covers both execution profiles.

## Accepted and Implemented Contract: Live Host Lifecycle

[ADR-0007](adr/0007-live-host-lifecycle.md) adds one terminal observation
method to the existing structured owner:

- `RuntimeTask::run_forever(&mut self)` waits for owner termination and returns
  its `ShutdownReport` or preserved `RuntimeError` after structured cleanup.
- It initiates no shutdown, closes no ingress, and chooses neither Drain nor
  Cancel. A healthy long-running program leaves the observation pending.
- The mutable borrow makes a losing `tokio::select!` observation
  cancellation-safe with respect to ownership. The same `RuntimeTask` remains
  available for an explicit `shutdown(Shutdown::Drain)` or
  `shutdown(Shutdown::Cancel)` call.
- Runtime completion can be selected directly against any host-owned future,
  so faults do not wait for an arbitrary sleep or later shutdown attempt.
- Samara accepts no host shutdown future and therefore cannot hide its result.
  Ctrl-C installation, supervisor errors, deadlines, and escalation remain
  host policy.
- A runtime fault racing a host shutdown condition remains visible either from
  `run_forever` or the subsequent shutdown boundary; selection adds no order
  between otherwise independent host events.

This contract freezes terminal observation and its ownership-cancellation
behavior. It does not add a live trace observer, default signal, default
shutdown mode, deadline, or automatic Drain-to-Cancel policy.

## Accepted and Implemented Contract: Shutdown Escalation

[ADR-0010](adr/0010-shutdown-escalation.md) adds
`RuntimeTask::request_shutdown(&mut self, Shutdown)`. It closes ingress
synchronously without waiting for cleanup. Running accepts either mode;
Draining may escalate to Cancelling. Repeated requests are harmless, Drain
cannot reverse Cancel, and requests after fault or closure preserve the
terminal result.

`run_forever` remains cancellation-safe observation of the same owner.
`shutdown(self, mode)` requests shutdown and awaits that same result, including
after an earlier request or observation. Fault arbitration and zero-owned-work
completion guarantees remain unchanged. Hosts choose grace periods; cancelling
an observation never resets shutdown or detaches work.

## Example-Driven Extension: Standard Output Effects

The first real-application experiment adds two narrow, high-level first-party
terminal EffectDescriptors:

- `PrintStdout` describes one finite, best-effort UTF-8 print to process
  standard output.
- `PrintStderr` describes one finite, best-effort UTF-8 print to process
  standard error.

Both descriptors own text, provide `text` for exact UTF-8 and `line` for UTF-8
followed by `\n`, produce `()`, and use `Infallible` as their Error type. The
values contain no handles and perform no work when constructed.

`LiveRuntimeBuilder::bind_stdio()` installs both Tokio-backed terminal Drivers.
Each Driver serializes its own Samara-issued print attempts within one live
binding, writes the complete text, and flushes the selected stream. An
operating-system I/O error is intentionally discarded rather than becoming an
application Message; `Succeeded(())` means the best-effort print attempt
finished, not that external observation is guaranteed. Direct process writes,
other runtime instances, independently issued effects, and stdout versus
stderr gain no ordering guarantee. Failure at the external stream or
whole-scope cancellation may leave partial output.

Controlled execution uses the ordinary `control_effect::<PrintStdout>()` and
`control_effect::<PrintStderr>()` boundaries and never touches the host process
streams. These descriptors add no formatting, logging, buffering, routing, or
retry policy to the runtime.

The root-qualified `samara::print!`, `samara::println!`, `samara::eprint!`, and
`samara::eprintln!` macros take the matching `EffectCapability` as their first
argument, accept familiar Rust formatting syntax, own the resulting `String` in
the matching descriptor, and return an ordinary
`Command::effect_discarding_outcome`. They are deliberately absent from
`samara::prelude`; qualification makes the deferred Samara effect visible at
the call site. The returned `Command` remains `#[must_use]` because dropping it
requests no output. By contrast, Rust's unqualified `print!` and `println!`
perform ambient process I/O immediately and must not be used inside a pure
Component transition.

A future lower-level exact-byte API may use names such as `WriteStdout` and
`WriteStderr`, expose `std::io::Error`, and make partial-write behavior part of
its contract. That fallible boundary is not implied by the high-level print
effects.

## Accepted and Implemented Contract: First-Party Standard Input Lines

[ADR-0009](adr/0009-first-party-stdin-lines.md) defines `StdinLines` as the
first-party terminal SourceDescriptor for process standard input. It is inert,
cloneable, and comparable; its Item is `String`, and its Error is
`StdinError`. `StdinErrorKind` distinguishes an operating-system `Read`
failure from `InvalidUtf8`, while `StdinError::new` lets controlled fixtures
construct either typed failure without touching the live process stream.

The descriptor emits one Item for each LF-delimited line. It removes the LF
and one immediately preceding CR, preserves empty lines, and emits nonempty
bytes before EOF as one final unterminated line. A read error or invalid UTF-8
terminates the Source with `SourceEvent::Failed` after any earlier complete
lines; clean EOF emits `SourceEvent::Ended` after the optional final Item. A
failure is not followed by `Ended`. The initial descriptor has no line-length
limit. Bounded or byte-oriented acquisition therefore requires another
terminal descriptor; a Layer above `StdinLines` sees only complete lines.

On Unix, `LiveRuntimeBuilder::bind_stdin()` discovers the sole declared
capability whose terminal descriptor is `StdinLines` and binds process stdin
to that exact capability. Zero or multiple matching declarations fail build.
The capability may be direct or a built-in composition such as
`SourceCapability<Framed<StdinLines, D>>`. A live runtime may contain only one
such binding, even for distinct capability values, and a second concurrent
realization faults rather than racing for lines or inventing broadcast
semantics. First-party bindings in separate live runtimes share that active
process lease. Once a realization releases the process reader, the same bound
capability may activate again at stdin's then-current position. Conforming host
code must not read process stdin concurrently with this binding.

Each active Unix realization owns an interruptible reader thread. Subscription
removal or replacement, Drain, Cancel, runtime fault, EOF, and typed failure
all cause that reader to be joined before its Source work is released. A
cutover that wins does not synthesize `Ended` or `Failed`. The live binding is
not claimed for non-Unix targets; the descriptor and controlled contract remain
portable, and another target may supply a custom Driver.

Input read and UTF-8 failures are typed Source failures. Failures in Samara's
private readiness, cancellation, or reader-thread mechanism instead fault the
runtime and cannot be mistaken for application input.

Controlled execution uses ordinary `control_source::<StdinLines>()` and
`emit_source` calls carrying `SourceEvent::Item`, `SourceEvent::Failed`, or
`SourceEvent::Ended`. It invokes no live stdin mechanism. EOF ends this Source;
it does not by itself choose whole-program shutdown policy.

## Example-Driven Extension: First-Party HTTP Effect

[ADR-0005](adr/0005-first-party-http-effect.md) adds one narrow raw HTTP
terminal EffectDescriptor:

- `HttpRequest` owns an `http::Method`, exact URL text, `http::HeaderMap`, and
  `bytes::Bytes` body.
- `HttpResponse` owns status, HTTP version, headers, and a fully buffered
  `bytes::Bytes` body.
- `HttpError` carries an `HttpErrorKind` distinguishing `Configuration` from
  `Transport`, plus explanatory text that controlled fixtures can construct.

`LiveRuntimeBuilder::bind_http()` installs one Driver retaining one reusable
reqwest client and connection pool. The Driver explicitly follows no redirects,
performs no protocol retries, discovers no system proxy, and performs no
automatic content decompression. It supplies no Samara request timeout. All
HTTP statuses—including 3xx, 4xx, and 5xx—produce a successful `HttpResponse`;
status interpretation belongs in the pure response pipeline, another future
pure Layer, or application logic.

If a descriptor omits `Accept`, the live Driver supplies `Accept: */*` as an
explicit no-preference transport default; a declared `Accept` value is
preserved. The Driver invents no other application-level request header.

The v0 Driver buffers the complete response body. It performs no JSON, text,
form, or content decoding; status conversion; authentication; cookies;
logging; retry; redirect; or application-specific header policy. Streaming,
limits, configurable clients, and those higher-level policies remain
deliberately unfrozen.

Controlled execution uses ordinary `control_effect::<HttpRequest>()`, exposes
the complete inert descriptor, and never constructs a live client or touches
the network. Mapped and discarded outcomes, Drain, Cancel, trace, and work
accounting retain the generic Effect semantics.

`HttpRequest::on_response()` consumes the request and returns a distinct
must-use `HttpResponsePipeline`. Request modifiers use `with_*` names and are
not available after that boundary. The response pipeline currently offers:

- `require_success()`, an explicit 2xx-only policy that retains the complete
  response in `HttpStatusError`; and
- `json::<T>()`, owned JSON decoding that does not imply status success and
  retains both the complete response and `serde_json::Error` in
  `HttpJsonError`.

A future request-side JSON encoder is reserved for a `with_json_body`-style
name. The response-side `json::<T>()` method always means decoding and never
modifies the request.

The non-exhaustive `HttpResponseError` distinguishes raw `HttpError`,
status-policy rejection, and JSON decoding while leaving room for later
explicit response operations. Cancellation remains `EffectOutcome::Cancelled`
rather than becoming response-error data. `into_command(&http)` uses the
canonical `Message: From<EffectOutcome<...>>` conversion, while
`into_command_with(&http, mapper)` accepts an explicit mapper. Both lower the
complete chain to one ordinary Effect Command for the original `HttpRequest`;
the pure response steps and application mapper each run at most once after the
raw terminal outcome.

Consequently, live assembly still binds only `HttpRequest`, controlled tests
still claim `next_effect::<HttpRequest>()`, and the generic trace records only
that terminal raw outcome. Status and JSON failures are deterministic mapper
behavior delivered through the resulting Component Message, not additional
runtime-traced Effect failures. A general `EffectPlan` and outer-outcome
tracing remain deferred.

## Accepted for ADR-0008: Closed Program Capabilities

[ADR-0008](adr/0008-closed-program-capabilities.md) closes Program assembly
around values issued by one `ProgramBuilder`:

- `effect::<D>() -> EffectCapability<D>` and
  `source::<S>() -> SourceCapability<S>` both declare a dependency and produce
  the only public authority that can issue work through that dependency.
- Component references and Ports remain the corresponding Program-issued
  values for concrete and provider-neutral Component communication.
- Component configuration stores these inert values. They expose no I/O,
  runtime handle, Driver, or controlled-world operation.
- `Command::effect(&capability, descriptor)`,
  `Command::effect_with(&capability, descriptor, mapper)`, and
  `Command::effect_discarding_outcome(&capability, descriptor)` are the only
  public Effect issuance paths.
- `Subscription::source(&capability, id, descriptor)` and
  `Subscription::source_with(&capability, id, descriptor, mapper)` are the only
  public Source issuance paths.
- Capability identity participates in Source reconciliation. Equal
  Subscription identity and descriptor with another Source capability replaces
  the Source.
- One `SourceCapability<Framed<S, D>>` declaration records the terminal `S`
  requirement. Applications do not separately declare or bind the inner
  descriptor or Layer stack.

After `ProgramBuilder::build()` consumes the builder, the logical capability
set is closed. `LiveRuntimeBuilder::build()` and
`ControlledRuntimeBuilder::build()` synchronously require exactly one
applicable terminal behavior for every declaration. Normal Drivers and
controlled registrations are type-wide; the one-shot `mpsc` bridge uses an
exact Source capability through `bind_mpsc(&capability, receiver)` and
`control_stream(&capability)`. Controlled `emit_stream` and `close_stream`
operations select that same exact capability. These exact APIs also accept a
composed capability such as `SourceCapability<Framed<StreamDescriptor<T>, D>>`;
raw items and ending enter at the terminal stream and traverse the declared
Layers. ADR-0009's Unix `bind_stdin()` is another exact live
binding, but it is unique across the process-input descriptor type rather than
one of several independently bindable receivers.

Raw descriptors cannot construct Commands or Subscriptions. HTTP pipeline
lowering, standard-output macros, and Source bridge helpers also require their
matching capability so a convenience API cannot reopen an undeclared path.
Initial foreign Effect, Source, ComponentRef, and Port capabilities and foreign
exact bindings fail profile build. A deliberately hidden foreign capability
first reached after execution starts is a Component-conformance fault rejected
before Driver or controlled behavior, not a supported form of dynamic assembly.

## Deliberately Unfrozen Surfaces

The following decisions remain explicit gates or deferrals rather than
accidental promises made by a placeholder type or variant:

- Request failure, deadline, per-Request cancellation, late-Reply,
  abandoned-Reply, and delegation policies beyond Phase 5's successful
  Component `Replied` path and ADR-0006's narrow live-host closure error.
- Notification delivery-failure semantics beyond accepted live admission and
  whole-scope Cancel/fault cutovers.
- The complete public Command/conformance inspection API, including sends,
  timers, batches, and stored message mappers.
- The detailed error taxonomy for closed logical assembly, complete
  profile-binding validation, and later foreign-capability conformance faults.
- General Layer and profile-binding abstractions beyond ADR-0008's type-wide
  Driver and exact Source-capability forms.
- Descriptor/message payload tracing, typed trace projections, streaming and
  public live observer APIs, and any durable trace representation. A public
  live observer is explicitly outside v0 under ADR-0004.
- Runtime error taxonomy beyond the observable missing-controlled-behavior and
  accepted live runtime-fault boundaries.
- Bounded queues, admission quotas, load shedding, coalescing, fairness,
  priorities, and a stable overload/backpressure API beyond v0
  unbounded internal delivery.
- Shutdown deadlines, grace periods, escalation, host-signal policy, and exact
  completed/cancelled diagnostic accounting beyond accepted Drain and Cancel.
- Per-effect deadlines, supersession, abandonment, and in-band cancellation
  beyond whole-scope termination.
- Automatic Source restart, retry, restartable or shared channel bridges, and
  Driver fault isolation or recovery.
- First-party bridge module/type names, provided implementation preserves
  ADR-0004's accepted observable EOF and bridge semantics. The accepted Phase 6
  audit froze the `Decoder::finish` spelling.

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
4. Every V1-V12 requirement maps to named scenarios and a planned phase.
5. Missing semantics and non-executable promises are listed rather than hidden
   behind passing placeholder tests.
6. Formatting, documentation, Clippy, and stale-vocabulary checks pass.

## Explicit HTTP Redirect Following

[ADR-0012](adr/0012-http-redirect-layer.md) adds the opt-in
`HttpResponsePipeline::follow_redirects(max_hops)` Layer. It runs unchanged in
live and controlled execution, emitting an individually visible `HttpRequest`
for each hop and mapping only the final outcome to an application Message.
Raw Driver behavior and pipelines without the opt-in remain unchanged. Private
pure Layer continuations may describe another finite Command without mutating
Component state; each controlled continuation is caused by its raw hop outcome.
