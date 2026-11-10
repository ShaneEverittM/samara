# Exploratory Design: Fluent HTTP Response Handling

Status: the narrow HTTP continuation-builder recommendation was accepted on
July 25, 2026 and is specified by
[ADR-0005](../adr/0005-first-party-http-effect.md), including its paired
`into_command` and `into_command_with` continuation API, and is implemented as
`HttpResponsePipeline`. This note now holds only the material that was deferred
and is recorded nowhere else: the general `EffectPlan` sketch, the response
error algebra shape, and the explicit punts and risks. The `EffectPlan` sketches
remain exploratory and are not architecture contracts. If later evidence
requires a runtime-traced composed outcome, write an EffectPlan ADR and
conformance contract before changing these semantics.

## Error algebra and transformation order

The terminal boundary, the declared-order rule, and the distinction among
transport failure, status policy, decoding failure, and cancellation are stated
in ADR-0005. Response policy can add a separate error algebra:

```rust
#[derive(Debug, thiserror::Error)]
pub enum HttpResponseError {
    #[error(transparent)]
    Http(#[from] HttpError),
    #[error(transparent)]
    Status(#[from] HttpStatusError),
    #[error(transparent)]
    Json(#[from] HttpJsonError),
}

pub struct HttpStatusError {
    response: HttpResponse,
}

pub struct HttpJsonError {
    source: serde_json::Error,
    response: HttpResponse,
}
```

The status and JSON errors should retain the raw response. This avoids throwing
away headers and diagnostic error bodies merely because a convenience policy
rejected or failed to decode them. They should provide `response()` and
`into_response()` accessors; `HttpJsonError` should additionally expose its
serde source.

## Future-compatible general EffectPlan shape

If evidence justifies general Effect Layers, the SourcePlan precedent suggests
this architecture.

### Consuming hidden lowering

Effect descriptors do not need `Clone`, so lowering must consume the value:

```rust
pub trait EffectDescriptor: Send + 'static {
    type Output: Send + 'static;
    type Error: Send + 'static;

    #[doc(hidden)]
    fn __samara_effect_plan(self, token: private::LowerToken) -> EffectPlan
    where
        Self: Sized,
    {
        EffectPlan::terminal(self)
    }
}
```

As with `SourcePlan`, the token and return type should be crate-private. Custom
downstream descriptors inherit terminal lowering; only Samara-owned composed
descriptors can override it. A public general custom-Layer extension point can
remain a separate later decision.

### Runtime-owned representation

Conceptually:

```rust
struct EffectPlan {
    declared_type_name: &'static str,
    terminal: Box<dyn ErasedTerminalEffectDescriptor>,
    layers: Vec<Box<dyn ErasedEffectLayer>>,
    output_type: TypeId,
    error_type: TypeId,
    valid_chain: bool,
}

trait ErasedEffectLayer: Send {
    fn map_result(
        self: Box<Self>,
        result: Result<Box<dyn Any + Send>, Box<dyn Any + Send>>,
    ) -> Result<Box<dyn Any + Send>, Box<dyn Any + Send>>;
}
```

The terminal descriptor is moved, not cloned, into the selected Driver or
`PendingEffect<HttpRequest>`. The runtime retains the ordered layers and the
application's optional one-shot Message mapper under the effect occurrence.
Completing the terminal descriptor consumes each layer exactly once, then
consumes the Message mapper at most once.

Cancellation should bypass `map_result` and remain
`EffectOutcome::Failed(EffectFailure::Cancelled(reason))` at every layer
(ADR-0020). This makes it impossible for
a status or decoding Layer to reinterpret runtime cancellation accidentally.
Built-in typed wrappers establish the input/output chain at compile time;
runtime TypeId validation remains defensive evidence at the erasure boundary.

### Controlled and live profiles

Controlled execution should continue to claim the terminal boundary:

```rust
let pending = runtime.next_effect::<HttpRequest>()?;
runtime.complete(pending, EffectOutcome::Succeeded(raw_response))?;
```

Completion then runs the retained ordered Layers and finally the application
mapper. Live execution binds the same terminal `HttpRequest` Driver; its raw
outcome enters the same retained Layers. Neither profile binds or executes a
Driver for the composed outer type.

### Tracing

A real EffectPlan requires a deliberate trace extension. At minimum, tracing
must distinguish:

- the declared composed effect type;
- the terminal descriptor type selected for controlled/live behavior;
- the terminal outcome supplied by the Driver or controlled harness; and
- the outer outcome after ordered Layers.

Otherwise a raw HTTP success transformed into a status or JSON failure would be
misreported. Whether individual Layer steps are traced is not yet necessary;
the terminal and outer outcomes are the salient semantic boundary.

### Discarded outcomes

`effect_discarding_outcome` on a composed effect should discard only the final
application Message mapper. The terminal descriptor remains runtime-owned, all
declared pure Layers run exactly once, the terminal and outer outcome kinds are
traced, and Drain/Cancel behavior is unchanged. Skipping Layers merely because
the Component discarded the outer outcome would make the composition's
semantics depend on observation and make traces misleading.

The narrow HTTP builder need not add a composed discard convenience in its
first slice. A caller that genuinely ignores an HTTP response can continue to
use `Command::effect_discarding_outcome(&self.http, HttpRequest::get(...))`;
decoding a value and then discarding it has no demonstrated ergonomic use yet.

## Explicit punts and risks

- Request JSON encoding (`with_json_body`) and its pure construction-error
  ergonomics.
- Text, form, multipart, streaming, and incremental response decoding.
- Response body limits and decompression.
- Content-Type validation.
- Typed preservation of response metadata on successful decoding.
- User-defined response transformations or a public custom Effect-Layer trait.
- General EffectPlan lowering, tracing, and public inspection.
- A composed discarded-outcome convenience.
- Whether JSON support should become feature-gated once Samara has a coherent
  crate-feature policy.

The main implementation risks in the narrow slice are accidentally treating
non-2xx as JSON failure, losing the raw response inside policy errors, obscuring
the fact that traces remain terminal/raw, and letting convenience APIs perform
serialization or other fallible work invisibly during `update`.
