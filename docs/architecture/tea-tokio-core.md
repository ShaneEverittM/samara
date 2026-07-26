# TEA + Tokio Core Architecture (v0)

## Status
- Phase: Phase 6 live-runtime implementation complete; ADR-0008 closed Program
  capabilities and ADR-0009 first-party stdin accepted for implementation.
- Date: July 25, 2026.
- Library scope: `samara` is library-first.

## Goals
- Combine Tokio async execution with strict The Elm Architecture (TEA) boundaries.
- Keep state evolution deterministic and inspectable.
- Keep side effects explicit, typed, and isolated from pure update logic.
- Support controlled execution, including faster-than-real-time progression for
  simulation and embedded-software testing scenarios.

## Non-Goals (v0)
- Durable event persistence and replay.
- A program-wide order for independent live events.
- Mandating a concrete mailbox, task, or event-loop topology.

## Core Contracts

### Model Contract
- `Model` represents all mutable domain state.
- No other Component or task may mutate `Model` directly.
- `Model` changes only through the runtime applying `update` output.

### Component Implementation and Configuration Contract
- A Component implementation is the Rust type and `impl Component` that define
  one kind of Component behavior.
- A configured value of that implementation contains immutable logical
  configuration and wiring, such as Ports, EffectCapabilities,
  SourceCapabilities, and SourceDescriptors.
- Component methods receive that value immutably. Behaviorally relevant mutable
  state belongs in `Model`; interior mutability must not create hidden state,
  effects, or transition inputs.
- Registering the configured implementation creates a logical Component with
  stable identity and a runtime-owned Model.

### Message Contract
- `Component::Message` is the only input that may trigger a Component
  transition.
- Domain events, user intents, timer callbacks, EffectOutcomes, SourceEvents,
  RequestOutcomes, and behaviorally relevant failures enter application logic
  as Component Messages.
- A Component Message and a Protocol Message are distinct roles even when both
  are represented by enums. Normative prose qualifies the role when ambiguity
  matters.

### Command Contract
- A `Command` is an inert value describing finite work requested by a
  transition; it never performs that work itself.
- Commands may carry a matching EffectCapability and EffectDescriptor with a
  one-shot message mapper or an explicit discarded-outcome mode, schedule a
  Message, communicate with another Component, issue a correlated Request, or
  emit a Reply.
- Command intent must remain explicit, inspectable, and testable in isolation.
- Command and EffectDescriptor are not synonyms: a Command is the broader
  finite-work envelope.

### Update Contract
- Conceptual semantic shape:

```text
update(&self, Model, Message) -> (Model, Commands<Message>)
```

- The Phase 3 Rust signature is
  `update(&self, &mut Model, Message) -> Command<Message>`. It uses exclusively
  owned in-place Model mutation as an observationally equivalent spelling of
  the conceptual transition and one composable Command value for finite work.
- Requirements:
  - Pure: no I/O, sleep, locks, random, wall-clock, global mutable reads/writes.
  - Deterministic: for equivalent immutable Component configuration, the same
    `model` + `message` must produce equivalent state and Command intent.
  - Total for supported messages: no silent drops.

### Closed Program Capability Contract

- `ProgramBuilder` is the only public issuer of ComponentRef, Port,
  `EffectCapability<D>`, and `SourceCapability<S>` values.
- Capability issuance is the dependency declaration. Components store these
  inert values in immutable configuration; no separate dependency manifest may
  drift from actual use.
- `ProgramBuilder::build()` closes the capability set. Execution may issue new
  occurrences and Model-derived descriptor values but may not add Components,
  Ports, Effects, or Sources.
- A raw EffectDescriptor cannot construct an Effect Command and a raw
  SourceDescriptor cannot construct a Subscription. Every public issuance path
  and first-party helper requires the matching capability.
- Live and controlled profile builders validate the complete declared terminal
  requirement set synchronously. Missing, duplicate, ambiguous, foreign, and
  type-incompatible bindings fail before execution.
- Normal Driver and controlled registrations are type-wide. Exact resource
  bridges such as one-shot Tokio `mpsc` and Unix process stdin bind one
  SourceCapability identity. Process stdin additionally permits only one live
  binding across capabilities.
- The closed inventory does not imply field reflection or static analysis of
  every behavior-dependent message edge. A deliberately hidden foreign
  capability faults before Driver or controlled behavior when first observed;
  it does not extend the Program dynamically.

### EffectDescriptor and EffectDriver Contract
- An `EffectDescriptor` is inert typed data describing one finite world-facing
  interaction. It need not be cloneable or comparable.
- An EffectDescriptor cannot hide execution in an opaque async closure; the
  terminal world interaction remains separately identifiable to the runtime.
- A Command combines an EffectDescriptor with either a pure, one-shot message
  mapper from `EffectOutcome` to the owning Component's Message or an explicit
  declaration that the Component discards the outcome. Discarding the outcome
  removes only the application continuation; it does not detach the effect
  from runtime ownership.
- `Command::effect(&capability, effect)` obtains that mapper from
  `Message: From<EffectOutcome<Output, Error>>` when the outcome type has one
  canonical Message meaning.
  `Command::effect_with(&capability, effect, mapper)` accepts an explicit mapper
  for call-site-specific meaning or captured domain context.
  Both forms create the same runtime-owned effect obligation and stored
  one-shot continuation.
- In live execution, an `EffectDriver<D>` is the terminal Adapter that realizes
  terminal descriptor type `D` against the surrounding world under runtime
  supervision. A non-terminal descriptor first passes through one or more
  Layers.
- Controlled execution supplies deterministic behavior for the same descriptor
  without invoking a live Driver or silently falling back to the live world.
- Each EffectOutcome actually accepted by a running scope either invokes its
  declared one-shot message mapper exactly once and returns the resulting
  Message through runtime-managed delivery, or closes an explicitly
  discarded-outcome obligation without scheduling a Message. A normally
  returning EffectDriver produces exactly one `Succeeded` or `Failed` outcome.
  ADR-0004 classifies whole-scope Cancel or runtime-fault cleanup as an abort
  instead: the future is cancelled, no EffectOutcome is manufactured, and no
  mapper is invoked. Drain never cancels an eligible finite effect merely to
  finish sooner.
- General profile-binding abstractions beyond ADR-0008's type-wide Driver and
  exact Source-capability forms remain an API decision.

### SourceDescriptor, Subscription, and SourceDriver Contract
- A `SourceDescriptor` is inert, typed, comparable configuration describing
  ongoing event production. Equality has reconciliation semantics.
- A `Subscription` combines a SourceCapability, stable Component-local
  identity, SourceDescriptor, and a pure reusable message mapper from
  `SourceEvent` to the owning Component's Message. Declaring one starts no work.
- `Subscription::source(&capability, id, descriptor)` obtains that mapper from
  `Message: From<SourceEvent<Item, Error>>`;
  `Subscription::source_with(&capability, ...)` accepts an explicit reusable
  mapper. This constructor choice does not participate in reconciliation
  identity or alter Source lifecycle semantics.
- In live execution, a `SourceDriver<D>` is the terminal Adapter that realizes
  terminal descriptor type `D` against the surrounding world under runtime
  supervision. A non-terminal descriptor first passes through one or more
  Layers. Controlled execution supplies deterministic behavior for the same
  terminal descriptor without a hidden live dependency.
- A `Source` is the runtime-owned ongoing realization of a SourceDescriptor. It
  may emit zero or more SourceEvents until it ends, fails, or is canceled.
- Reconciliation starts a Source for a newly desired Subscription, retains it
  while identity, SourceCapability, and descriptor are unchanged, atomically
  replaces it when the same identity has changed capability or configuration,
  and cancels it when no longer desired.
- A retained Source atomically adopts the latest mapper returned by the
  post-transition Subscription projection. Messages already created remain
  unchanged; later events use the new mapper.
- Each Source realization has a private runtime generation. Descriptor
  replacement is a hard cutover: old-generation events or mapped Messages that
  have not begun a Component transition are discarded and traced. An
  already-running transition completes, and no old event is mapped through the
  replacement generation.
- During reconciliation, a composed SourceDescriptor automatically lowers to a
  runtime-owned `SourcePlan` containing its terminal descriptor, ordered
  profile-independent Layers, Source capability identity, and message mapper.
  One outer SourceCapability records its terminal requirement; applications
  bind or control only the terminal descriptor.
- Source cancellation does not imply a synthetic SourceEvent unless the
  applicable contract explicitly promises one.
- Under ADR-0004, successful sink calls from one live Source preserve
  acceptance order. The first accepted `end`, accepted `fail`, or active
  SourceDriver return wins exactly one terminal event. Cancellation,
  replacement, shutdown, or runtime fault winning first suppresses terminal
  mapping; later sink calls return `DriverStopped`.
- Normal Source termination does not automatically restart a still-desired
  Subscription. The Source remains inactive until application desire changes
  in a way an explicit future restart policy recognizes.

### Adapter, Layer, and Driver Contract
- `Adapter` is the conceptual umbrella for code that connects declared
  boundaries; no universal `Adapter` trait is required.
- A `Layer` is a compositional Adapter. It remains inside the declarative
  boundary, behaves the same in live and controlled execution, and may own
  deterministic runtime-scoped state, but it performs no ambient I/O.
- A `Driver` is a terminal Adapter. It crosses from a descriptor stack into the
  selected live surrounding world and remains owned by the runtime scope.
- EffectDriver and SourceDriver are the currently named Driver roles. The
  taxonomy does not choose a universal Layer trait. ADR-0008 selects type-wide
  Driver bindings and exact Source-capability bindings for resource adapters.
- Runtime-level mechanisms remain separate from protocol and application
  policy. See `docs/architecture/effects-layering.md` for the underlying
  mechanism/policy analysis.

### Runtime Contract
- Runtime responsibilities:
  - Own message ingestion and routing.
  - Serialize transitions for each Component without requiring global serialization.
  - Execute each Component's `update` transition.
  - Interpret Commands, compose descriptors through zero or more Layers, and
    route only terminal descriptors to live Drivers or controlled behavior.
  - Apply message mappers to EffectOutcomes, SourceEvents, and RequestOutcomes.
  - Reconcile desired Subscriptions with runtime-owned Sources.
  - Compile composed SourceDescriptors into SourcePlans and enforce private
    Source-generation cutovers.
  - Reject Command and Subscription capabilities that do not belong to the
    Program before invoking terminal behavior.
  - Record topology-neutral structural trace entries with logical time and
    causation in controlled execution.
  - Supervise task lifecycle, cancellation, and shutdown.
  - Deliver resulting Messages to their target Components.
- In the Phase 6 live profile, own one admission boundary, close it on
  shutdown or runtime fault, avoid intentional capacity drops after successful
  acceptance while the scope is healthy, and ensure fault or shutdown joins or
  aborts every owned task.
- Only runtime-managed machinery may deliver messages or execute commands;
  application Components express coordination through messages.
- Runtime driving APIs should support condition-based execution (`run_until(...)` / `run_until_predicate(...)`) and quiescence execution (`run_until_idle()`), so tests/simulations do not depend on hard-coded wall-clock sleeps.

### Component Interaction Contract (`send` / `notify` / `request`)
- Every cross-Component interaction is an explicit Command; constructing one does
  not invoke another Component during the current transition.
- `Command::send` is the lower-level one-way form for deliberate coupling to a
  target `ComponentRef<C>` and its complete `C::Message` vocabulary.
- Provider-neutral interaction uses a named `Port<P>`:
  - A value implementing `Notification<P>` is issued through `Command::notify` for
    one-way delivery.
  - A value implementing `Request<P>` declares one associated `Reply` type and
    is issued through `Command::request` or `Command::request_with` for a
    correlated terminal outcome.
- `Command::request(port, request)` obtains its pure one-shot continuation from
  `Message: From<RequestOutcome<Reply>>`. `Command::request_with(port, request,
  mapper)` accepts an explicit continuation, including one that captures
  application-owned domain correlation. Both forms create the same Request
  obligation and runtime-owned transport correlation.
- The runtime owns transport correlation and creates an opaque, typed
  `ReplyTo<Reply>` when it interprets a request. Components do not allocate,
  compare, or retain transport correlation identifiers.
- A provider receives one `RequestInvocation<P, R>` containing the typed Request
  and its inert `ReplyTo<R::Reply>`, then emits `Command::reply`. Consuming the
  authority expresses at-most-once reply capability without exposing a channel,
  future, or runtime handle inside `update`.
- The requester never awaits inside `update`; the mapped `RequestOutcome` returns
  through normal runtime-managed message delivery.
- Phase 5 implements the successful path only: typed Port delivery, one opaque
  transport-correlation token, at-most-once Reply, mapping to
  `RequestOutcome::Replied`, causal tracing, and one outstanding-Request
  obligation. An unanswered Request remains pending until controlled
  cancellation cleans up runtime ownership.
- Request failure, abandonment, timeout, cancellation, late-Reply, and
  delegation semantics remain deferred; Phase 5 does not manufacture those
  outcome variants.
- [ADR-0006](../adr/0006-live-port-ingress.md) adds a live-only host boundary.
  `LiveRuntime::port_handle` validates one exact built `Port<P>` binding and
  returns a cloneable `PortHandle<P>` with `notify` and `request` operations.
- PortHandle uses the same Protocol conversion, provider Message delivery,
  RequestInvocation, opaque transport correlation, and `Command::reply` path as
  Component-issued Port work. It cannot access the provider or Model and never
  invokes a transition directly.
- A host Request awaits `Result<R::Reply, RuntimeError>` directly. It has no
  Component Message continuation and does not construct RequestOutcome. This is
  an external Tokio completion boundary, not a future available inside
  `update`.
- PortHandle is live-only. ControlledRuntime continues to expose deterministic
  input and drive operations rather than a live host handle.

### Component Decoupling Contract (`Port` / protocol binding)
- Reusable Component collaboration should prefer protocol-level Ports over
  concrete Component message coupling.
- A `Protocol` owns a provider-neutral `Protocol::Message` vocabulary independent
  of any provider Component's private `Component::Message` type. A concrete
  protocol enum should make that role clear, for example
  `HealthProtocolMessage`.
- A `Port<P>` is an inert, named logical dependency. It contains no provider
  reference, channel, runtime handle, or lookup capability.
- A `PortHandle<P>` is a distinct live capability obtained from an assembled
  LiveRuntime for one validated Port binding. It must not be stored in Component
  configuration or treated as logical wiring.
- Program assembly binds each exact named Port to a provider Component whose
  Message implements `From<Protocol::Message>`. This standard conversion is
  pure and reusable rather than supplied repeatedly as a binding closure.
  Multiple named Ports of the same Protocol may be bound independently.
- `ProgramBuilder::build()` is fallible. It rejects duplicate Component
  identities, duplicate `(Protocol type, PortId)` declarations, Ports not bound
  exactly once, and providers not registered in that same builder.
- Port cycles are legal and are not detected. Assembly closes the Program-issued
  capability inventory without introspecting arbitrary Component fields or
  statically enumerating behavior-dependent `Command::send` edges. Initial
  foreign ComponentRef and Port commands fail profile build; a foreign
  reference hidden until execution is diagnosed before its send is interpreted.
- Swapping a real, mock, or controlled provider does not require consumer
  transition changes.
- Notification values and Request values use the symmetric `Command::notify`
  and `Command::request` entry points; only Requests add an associated Reply and
  continuation. `Command::request_with` is the explicit-mapper spelling of the
  same Request operation.
- Runtime-owned routing and reply resolution must preserve the delivery,
  causality, and failure semantics promised by the interaction contract without
  exposing runtime topology.

### Live Admission and Internal Pressure Contract

- Under ADR-0004, successful `ComponentHandle::send` means accepted
  for runtime-managed delivery, not that the target transition completed.
- Under ADR-0006, ComponentHandle and PortHandle share one atomic external
  admission cutoff. A PortHandle future attempts admission when first polled.
  Successful notify means accepted, not provider transition completion; a
  request that is waiting for Reply has already been admitted and is
  runtime-owned.
- Shutdown or runtime fault closes external admission. A racing Component send,
  Port Notification, or Port Request is either accepted under the chosen closure
  policy or rejected explicitly.
- The v0 live runtime uses unbounded internal delivery while the scope is
  healthy and running. Accepted work is not intentionally dropped because an
  internal queue filled.
- This supplies no stable capacity, fairness, throughput, latency, or
  backpressure guarantee. Sustained overload may grow memory without bound;
  memory exhaustion is unsupported.
- An upstream bounded Tokio `mpsc` channel retains its own pressure only until
  Samara receives an item. Samara adds no second bounded-pressure contract.

### Time and Controlled-Execution Contract
- The runtime must provide a clock/scheduling abstraction boundary that can support:
  - Real-time execution.
  - Logical-time execution controlled by tests or simulation harnesses.
  - Faster-than-real-time progression when controlled conditions allow.
- Timer semantics must be representable as explicit commands so scheduling can be mediated by the runtime boundary.
- Public runtime APIs must avoid forcing hard wall-clock coupling that would prevent time acceleration.
- Deterministic controlled runs must be possible with controlled clock progression.
- The v0 controlled scheduler orders runnable work by `(logical deadline,
  deterministic insertion ticket)`. Commands from one transition follow
  declaration traversal order; controlled inputs follow harness order; initial
  Component work is canonicalized by `ComponentId`, never registration order;
  and causally emitted work follows its cause.
- This equal-time rule is controlled-runtime reproducibility machinery, not a
  live or domain ordering guarantee. Cross-profile ordering requirements must
  be expressed as explicit causality.

### Controlled Trace and Work-Accounting Contract

- Controlled execution always records an in-memory structural trace that tests
  read after driving; Phase 5 requires no scheduler callback.
- Each record has a `TraceId`, `LogicalTime`, and `TraceEvent`. Initialization
  and controlled harness inputs are roots with no parent; every other record
  has exactly one immediate causal parent.
- The trace explains Component transitions; Command kinds, concrete descriptor
  types, and targets; Subscriptions and Source lifecycle including stale drops;
  terminal outcome/event shapes; logical time; and causation.
- The generic trace need not capture domain payloads. Direct typed-intent tests
  compare descriptor and Message payload values. Typed trace projection,
  streaming observers, durable storage, and replay are deferred.
- `pending_now` counts accepted Component Messages and due timers ready to run.
  `pending_later` counts pending effects, future timers, active Sources (one
  each), and outstanding Requests. Tasks, queues, locks, interpreter steps, and
  trace records are not separate obligations.
- A successful `run_until_idle()` normally returns with `pending_now == 0`;
  `pending_later` may remain nonzero. Controlled cancellation reduces both to
  zero.
- If interpretation reaches a terminal descriptor without controlled
  behavior, the drive operation fails there without invoking a live Driver or
  message mapper and records Component, work-occurrence, and descriptor-type
  context. The run is faulted: state and trace remain inspectable and
  cancellation remains available, but driving cannot resume.

### Initial Live Shutdown Contract

- Drain atomically closes external ingress, disables Source start and
  restart, stops active Sources, and recursively processes Component Messages
  and Source deliveries accepted before the cutoff plus finite work and timers
  causally emitted while draining.
- Future timers, finite effects, and Requests remain eligible. A hung Driver,
  unanswered Request, distant or recurring timer, or self-sustaining
  application may keep Drain pending forever.
- Drain retains Port Notifications and host Requests admitted before its shared
  cutoff. It cannot report clean completion while a host Request remains
  outstanding, even if the host waiter was dropped.
- Cancel closes ingress, stops application driving, cancels queued and
  deferred semantic obligations and runtime-owned Driver tasks, then joins or
  aborts all owned tasks. It does not invoke application mappers solely because
  the scope ended.
- Cancel or non-fault closure before a host Reply wakes the external waiter with
  `RuntimeError`, not RequestOutcome. Dropping an unpolled host request admits
  nothing; dropping an admitted waiter relinquishes observation but does not
  cancel or remove runtime-owned work.
- Successful shutdown reports
  `remaining == pending_now == pending_later == 0`. Completed and cancelled
  diagnostics count semantic obligations rather than tasks or queues, but
  their exact values remain non-normative in v0.

### Initial Live Fault Contract

- Every declared Effect and Source capability must have exactly one applicable
  terminal live binding. Missing, duplicate, ambiguous, foreign, or
  type-incompatible bindings fail `LiveRuntimeBuilder::build()`.
- An exhausted one-shot `mpsc` binding, a Driver panic, a deliberately hidden
  foreign capability reached after startup, or an equivalent live mechanism
  violation faults the running scope. A legitimate declared dependency cannot
  first discover its binding is missing during execution.
- A live fault closes ingress, stops application driving, cancels and joins or
  aborts all runtime-owned tasks, suppresses application mappers for aborted
  work, and surfaces `RuntimeError` through subsequent ingress and the owning
  `RuntimeTask::run_forever` or `RuntimeTask::shutdown` boundary.
- A fault that wins before a host Port Reply wakes that waiter with the same
  preserved RuntimeError and causes later PortHandle operations to return that
  fault rather than a generic closure error.
- Fault cleanup does not invent typed application Error payloads. Exact fault
  taxonomy, isolation, restart, and recovery remain provisional.

### Live Host Lifecycle Contract

- `RuntimeTask::run_forever(&mut self)` observes terminal owner completion
  without initiating shutdown or choosing a policy. A runtime fault becomes
  visible as soon as structured fault cleanup completes.
- Cancelling that observation leaves `RuntimeTask` owning the live scope. This
  permits ordinary `tokio::select!` composition followed by explicit Drain or
  Cancel; observation cancellation itself closes no ingress and detaches no
  work.
- Samara installs no OS signal and accepts no host shutdown future. Signal
  errors, supervisor protocol, deadlines, and escalation remain host concerns.
- A fault racing a host condition is preserved through either the observation
  result or subsequent shutdown. This terminal race adds no global or domain
  order guarantee.

### First-Party Phase 6 Bridge Contract

- First-party TCP owns one connection per Source realization, emits
  `bytes::Bytes`, maps connect/read errors to typed Source failure, maps peer
  EOF to normal ending, closes on cancellation, and contains no retry,
  reconnect, framing, or domain policy.
- One `tokio::sync::mpsc::Receiver<T>` is bound through
  `bind_mpsc(&source_capability, receiver)` and consumed by the first
  activation of that exact capability. Its terminal descriptor must be
  `StreamDescriptor<T>`; the capability may describe that terminal directly or
  a built-in composition such as `Framed<StreamDescriptor<T>, D>`. Items and
  channel closure traverse the same Layers in both profiles, and closure ends
  normally.
  Another concurrent claimant or any later activation after end or cancellation
  faults the runtime; it does not fabricate another `Ended`.
- The `mpsc` rule is necessarily diagnostic because
  `StreamDescriptor<T>::Error` is `Infallible` and no new receiver exists to
  realize the repeated desire.
- Exact first-party public module/type names remain implementation-review
  details where the accepted API has not already frozen them.

### First-Party Standard Input Line Source

- [ADR-0009](../adr/0009-first-party-stdin-lines.md) defines `StdinLines` as a
  terminal SourceDescriptor with `String` Items and `StdinError` failures.
  `StdinErrorKind` distinguishes operating-system `Read` failure from
  `InvalidUtf8`; controlled fixtures can construct either typed value.
- One Item represents one LF-delimited line. The LF and one immediately
  preceding CR are removed, empty lines are preserved, and nonempty bytes at
  clean EOF produce one final unterminated Item before `Ended`.
- Read or UTF-8 failure terminates the Source with `Failed` after any earlier
  complete valid lines and is not followed by `Ended`. The v0 descriptor has
  no line-length limit.
- On Unix, `bind_stdin(&capability)` is an exact live binding for one capability
  whose terminal descriptor is `StdinLines`. Direct and built-in composed
  capabilities such as `Framed<StdinLines, D>` use the same terminal binding.
  A runtime accepts only one stdin binding even for distinct capabilities, and
  bindings in separate live runtimes share the process-wide active lease. A
  second concurrent realization faults rather than sharing or broadcasting
  the process stream; a later sequential realization may resume at stdin's
  current position.
- Conforming code outside Samara does not read process stdin concurrently with
  this binding. The first-party live realization is not promised on non-Unix
  targets; the descriptor and controlled event contract remain
  platform-neutral.
- Each active Unix realization owns an interruptible reader thread. Removal,
  replacement, Drain, Cancel, runtime fault, EOF, and typed failure signal and
  join that reader before releasing the Source. A cutover emits no synthetic
  terminal event.
- Input read and UTF-8 errors are typed Source failures; failures in the private
  readiness, cancellation, or reader-thread mechanism are runtime faults.
- Controlled execution uses ordinary `control_source::<StdinLines>()` and
  `emit_source` with typed `SourceEvent` values. It neither opens process stdin
  nor invokes the live reader. EOF ends this Source but does not implicitly
  shut down the Program.

### First-Party Standard Output Effects

- `PrintStdout` and `PrintStderr` are finite terminal EffectDescriptors, not
  ambient capabilities available to `update`.
- Live assembly opts into their Tokio Drivers through `bind_stdio`; controlled
  execution intercepts the same descriptors through the normal typed effect
  boundary.
- They are best-effort text effects with `Infallible` application Error data.
  The Driver attempts the complete write and flush but deliberately discards
  host I/O errors; cancellation may leave partial external output.
- The Drivers contain no formatting, logging, retry, or application routing
  policy and create no ordering guarantee between otherwise independent
  effects.
- A future fallible exact-byte write boundary is a separate, lower-level API.
- Root-qualified `samara::print!`, `samara::println!`, `samara::eprint!`, and
  `samara::eprintln!` take the matching output EffectCapability first, own their
  formatted UTF-8 text in the matching descriptor, and return an ordinary
  discarded-outcome Command. They are not re-exported by the prelude. Rust's
  unqualified `print!` and `println!` remain immediate ambient I/O and are not
  valid inside a pure `update`.

### First-Party HTTP Effect

- [ADR-0005](../adr/0005-first-party-http-effect.md) defines `HttpRequest` as a
  finite terminal EffectDescriptor containing owned method, URL text, headers,
  and body bytes.
- Its raw `HttpResponse` output contains status, version, headers, and a fully
  buffered body. Every HTTP status is an output; the Driver does not turn 3xx,
  4xx, or 5xx into failure.
- `HttpError` distinguishes descriptor/client configuration from live
  transport failure. Controlled fixtures can construct the same typed value.
- `bind_http` owns one reusable reqwest client and pool for the binding. It
  disables redirects, retries, system proxies, and automatic content
  decompression and supplies no request timeout.
- The live Driver supplies the no-preference transport default `Accept: */*`
  only when the descriptor omits `Accept`; an explicit value is preserved.
- JSON decoding, status policy, authentication, retry, redirect, logging,
  streaming, and application header behavior do not belong in this terminal
  Driver. The pure response pipeline, future Layers, or application logic may
  add those semantics explicitly.
- Controlled execution intercepts `HttpRequest` through the normal typed effect
  boundary and never falls through to the live network.
- `on_response` is an explicit consuming phase boundary from request
  construction to a must-use pure response pipeline. Request `with_*` methods
  are not part of the response-pipeline type.
- `require_success` adds only an explicit 2xx policy. `json::<T>` adds only
  owned JSON decoding and never implies that policy. Both retain the complete
  raw response in their typed error data; JSON errors also retain their source.
- `into_command(&http_capability)` uses
  `Message: From<EffectOutcome<Output, ResponseError>>` for the final
  conversion; `into_command_with(&http_capability, mapper)` accepts an explicit
  call-site mapper.
  Each lowers the pipeline to the original terminal `HttpRequest` plus one
  composed `FnOnce` mapper. Live and controlled execution therefore share the
  same deterministic status/decoding behavior without a new Driver, controlled
  boundary, runtime event, Command kind, or trace outcome.
- Raw configuration/transport failure and runtime cancellation bypass response
  operations. Cancellation remains cancellation rather than response Error
  data.
- A general EffectPlan, user-defined response transforms, and runtime tracing
  of the composed outer result remain deferred.

### Decoder EOF Contract

- ADR-0004 adds one required pure Decoder finalization operation.
- Only normal underlying `Ended` invokes finalization, exactly once and after
  all earlier accepted chunks from that Source.
- Final frames are emitted in order before outer `Ended`. A finalization Error
  emits one `FramedError::Decode` failure and no `Ended`.
- Underlying Source failure, an earlier decoder failure, cancellation,
  replacement, shutdown, and runtime fault do not invoke finalization.
- First-party TCP chunks use `bytes::Bytes`; decoder state may use
  `bytes::BytesMut`.

## Error Channel Design
- Error names typed explanatory data, such as `TcpError`, `StdinError`, or
  `RequestError`.
  Failure names the semantic occurrence carrying Error data. Normal Source
  ending and cancellation are distinct terminal conditions, not failures.
- Expected behaviorally relevant failures for which a boundary contract defines
  typed Error data must be representable as Component Messages,
  EffectOutcomes, SourceEvents, or RequestOutcomes where they occur.
- A live host Port Request is outside Component application logic. Its successful
  terminal value is `R::Reply`; whole-scope closure or fault is reported through
  `RuntimeError`. That host diagnostic does not manufacture an in-band
  RequestOutcome or weaken Message-only Component state transitions.
- Driver panics and live mechanism faults cannot truthfully construct arbitrary
  typed application Error data. ADR-0004 surfaces them as
  `RuntimeError` after initiating structured scope cleanup.
- The exact boundary variant taxonomy remains a focused API decision.
- No panic-based control flow for expected errors.

## Message, Effect, and Source Lifecycles
1. A Component Message enters runtime-managed delivery for a target Component.
2. The runtime selects it according to per-Component serialization, causality,
   and any explicitly promised sequencing contract.
3. The runtime calls the target Component's pure transition using its immutable
   configuration, current Model, and Message, then commits the resulting Model.
4. The runtime interprets emitted Commands and reconciles desired Subscriptions.
5. The runtime first validates the Command's EffectCapability provenance. Its
   EffectDescriptor then passes through zero or more Layers. The terminal
   descriptor reaches a live EffectDriver or controlled behavior. Every
   accepted EffectOutcome passes through the composed path exactly once. A
   mapped effect invokes its one-shot mapper to produce one Component Message;
   an explicitly discarded outcome schedules no Message. Whole-scope live abort
   may instead cancel the Driver without manufacturing an outcome.
6. The runtime validates each desired Subscription's SourceCapability. Its
   SourceDescriptor then passes through zero or more Layers. The terminal
   descriptor, ordered Layers, capability identity, and mapper compile into a
   SourcePlan. The terminal descriptor reaches a live SourceDriver or
   controlled behavior, producing a runtime-owned Source whose SourceEvents
   return through the same Layers and repeatedly pass through the current
   reusable message mapper.
7. Resulting Messages return through runtime-managed delivery, and execution
   continues until the applicable lifecycle, fault, or shutdown policy
   triggers.

## Topology Status
- Runtime topology is not prescribed for v0.
- A single loop, one loop per Component, a hybrid scheduler, or another design may conform.
- Each Component's transitions are serialized; causal and explicitly promised sequencing guarantees are preserved.
- Independent live events have no implicit program-wide order.
- Controlled execution selects a deterministic schedule and produces a reproducible program-wide trace.
- ADR-0003 defines the v0 controlled equal-time schedule, Source cutover,
  structural trace, validation, and semantic work-accounting rules.
- ADR-0004 defines no extra global live order. Phase 6 evidence accepts
  either order for independent completions and compares causal partial order,
  not controlled trace-vector order.
- Internal topology may change without an ADR when these observable semantics remain unchanged.
- See `docs/architecture/topology-options.md`,
  `docs/adr/0002-runtime-topology-and-ordering.md`, and
  `docs/adr/0003-controlled-execution-semantics.md`. Accepted live behavior is
  in
  `docs/adr/0004-initial-live-runtime-semantics.md`; closed capability assembly
  is in `docs/adr/0008-closed-program-capabilities.md`.

## Related Design Sketches
- Thin-slice API comparison and PoC shape: `docs/architecture/thin-slice-value.md`.
- Effect mechanism/policy split: `docs/architecture/effects-layering.md`.
