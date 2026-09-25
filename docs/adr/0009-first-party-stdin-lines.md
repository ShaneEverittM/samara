# ADR-0009: First-Party Standard-Input Line Source

- Status: Accepted
- Date: July 25, 2026
- Deciders: Samara maintainers
- Scope: First-party interactive standard-input ingress for live and controlled execution
- Amended: September 24, 2026 — discover the stdin capability during live build

## Context

The first real application example needs an ongoing stream of terminal
commands. Its initial version created a host-owned blocking reader thread,
forwarded lines through a Tokio mpsc channel, and bound that receiver through
`StreamDescriptor<String>`. The Component remained conforming, but every
application wanting terminal input would need to repeat the same adapter,
shutdown, EOF, and error policy.

Using `tokio::io::stdin()` does not solve the lifecycle problem. Tokio
implements standard input with an ordinary blocking read on a helper thread and
documents that the read cannot be cancelled; runtime shutdown may wait until
the user presses Enter. Detaching an application-created reader thread would
make Ctrl-C appear responsive, but would violate Samara's structured-ownership
goal by leaving world-facing work outside the runtime scope.

Standard input is also a unique process resource. Treating it as an ordinary
type-wide `SourceDriver<StdinLines>` would permit several concurrent Source
realizations to race for lines, even though no useful broadcast or arbitration
semantics had been declared.

## Decision

Samara provides a first-party terminal SourceDescriptor named `StdinLines`.

```rust,ignore
let stdin = program.source::<StdinLines>();

Subscription::source_with(
    &stdin,
    SubscriptionId::new("commands"),
    StdinLines,
    |event| match event {
        SourceEvent::Item(line) => Message::Line(line),
        SourceEvent::Failed(error) => Message::InputFailed(error),
        SourceEvent::Ended => Message::InputEnded,
    },
)

let runtime = LiveRuntime::builder(program)
    .bind_stdin()
    .build()?;
```

`StdinLines` is inert, cloneable, and comparable. Its Item is `String`; its
Error is `StdinError`. `StdinErrorKind` distinguishes an operating-system read
failure from invalid UTF-8, and `StdinError::new` makes the same typed error
constructible in controlled tests.

The line contract is:

- one Item is emitted for each LF-delimited line;
- the terminating LF and one immediately preceding CR are removed;
- empty lines produce an empty String;
- nonempty bytes before EOF form one final Item even without a terminating LF;
- invalid UTF-8 terminates the Source with `Failed`, after any earlier complete
  valid lines;
- an operating-system read error terminates the Source with `Failed`; and
- clean EOF terminates the Source with `Ended` after the optional final Item.

There is no line-length limit in the first cut. Applications requiring bounded
or byte-oriented acquisition need a separate descriptor rather than a silent
policy inside `StdinLines`; a Layer above `StdinLines` can apply policy only
after each complete line has been acquired.

### Exact live binding and uniqueness

`LiveRuntimeBuilder::bind_stdin()` requests binding process stdin to the
Program's sole declared capability whose terminal descriptor is `StdinLines`.
`build()` discovers that capability and creates the same exact binding used
by the reader. Zero or multiple matching declarations fail build, including
unused declarations and mixtures of raw and composed stdin capabilities.
Repeated `bind_stdin()` calls and conflicts with a type-wide stdin Driver also
fail build. Registration and build do not acquire or read process stdin.

This replaces the capability argument: the single-resource rule already
forbids multiple choices, so applications need not expose their input capability
just for host binding. Unlike type-wide HTTP binding, this does not authorize
several independent stdin capabilities or introduce broadcast behavior.
The application-visible capability may be `SourceCapability<StdinLines>` or a
built-in composition such as `SourceCapability<Framed<StdinLines, D>>`; the
terminal binding still sees lines before the pure Layers run. The method is
intentionally separate from `bind_stdio()`, which continues to opt into only
the stdout and stderr Effects.

Only one stdin binding may exist in a live runtime, even for distinct
capabilities. A Program declaring more than one stdin capability therefore
cannot satisfy live build validation. Cloning one capability is allowed as
ordinary immutable wiring, but a second concurrent Source realization faults
the runtime because process stdin already has an active owner. First-party
bindings in separate live runtimes share that process-wide active lease. After
a removed, replaced, failed, or ended realization has released the reader, the
same bound capability may activate again at stdin's then-current position.
Samara does not invent broadcast semantics.

The first-party live binding is initially available on Unix. Controlled
execution and the public descriptor/error vocabulary remain platform-neutral;
other targets may use a custom Driver until an equally cancellable first-party
implementation is specified.

Conforming host code must not read process stdin concurrently with this
binding. Samara can enforce exclusive ownership among its declared
capabilities, but it cannot prevent ambient code or another library from racing
the readiness notification and blocking on the same process resource.

### Structured cancellation

On Unix, the live binding owns one interruptible reader thread inside the
Source realization. The reader waits for either stdin readiness or a private
cancellation channel. Removing or replacing the Subscription, Drain, Cancel,
and runtime fault signal that channel and join the reader before the Source
future is released. EOF and failure also join it normally. No reader thread is
detached, and cancellation emits no fabricated terminal Source event.

The helper thread is terminal Driver mechanism: it owns no Component state,
performs no command parsing, and communicates only typed line/failure/EOF data
through the runtime-owned Source sink.

### Controlled execution

Controlled execution uses the ordinary `control_source::<StdinLines>()` and
`emit_source` APIs, carrying `SourceEvent::Item`, `SourceEvent::Failed`, or
`SourceEvent::Ended`. It never opens process stdin or invokes the live binding.
Tests can inject lines, failures, and EOF in a deterministic program-wide
order.

## Consequences

### Positive

- Components declare terminal input as an ordinary ongoing Source.
- Applications no longer hand-roll a blocking thread plus mpsc adapter.
- Live and controlled programs use the same descriptor and message mapping.
- The unique process resource has explicit capability ownership and build-time
  binding validation.
- Unix cutover preserves structured ownership instead of relying on Enter to
  release Tokio's uncancellable stdin read.

### Negative

- The first-party live binding is initially Unix-only.
- Line buffering is unbounded, matching common line-reader behavior but not
  every hostile-input requirement.
- One process stdin cannot be consumed independently by several Components;
  applications needing fan-out must place it behind one owning Component.
- Runtime idleness after EOF is not whole-program shutdown. Host shutdown
  policy remains outside the Component unless a later explicit application
  lifecycle protocol is designed.

## Failure Modes

- Missing bindings, zero or multiple stdin declarations with `bind_stdin()`,
  and duplicate or conflicting stdin bindings fail live build. Discovery uses
  only the runtime's own Program; no foreign binding argument is accepted.
- A second concurrent realization of the stdin capability faults the runtime.
- Read and UTF-8 failures become one typed terminal Source failure.
- Private readiness, cancellation-channel, and reader-thread failures fault
  the runtime rather than masquerading as typed input failures.
- EOF becomes one normal Source end.
- Cancellation wins without synthesizing `Ended` or `Failed`.
- A panic or failure in the reader mechanism faults the runtime and performs
  structured cleanup.

## Required Evidence

- Descriptor and error values expose the frozen typed contract.
- Controlled tests inject line, failure, and EOF events without live I/O.
- Live tests with an injected Unix stream prove line splitting, CRLF removal,
  final unterminated-line delivery, invalid UTF-8 failure, and EOF ordering.
- A live cancellation test proves an idle reader is interrupted and joined
  without requiring another input byte.
- Missing bindings, zero/multiple declarations, duplicate requests, type-wide
  conflicts, concurrent realization, and sequential reactivation behave at the
  stated boundary. Raw and composed capabilities use the same discovery rule.
- A composed capability whose terminal descriptor is `StdinLines` uses the
  same exact binding.
- The real application example contains no stdin thread or mpsc binding and
  uses `StdinLines` unchanged in its Component logic.

## Rollback

To revert automatic selection, restore the explicit capability argument and
pass it through application assembly. Exact ownership, controlled execution,
ordering, cancellation, and reader cleanup are unchanged by this amendment.

Remove `StdinLines`, its error types, the exact live binding, and its tests;
restore the example's explicit host adapter. No Component Model, Message,
Effect, Port, or general Source semantics need change.
