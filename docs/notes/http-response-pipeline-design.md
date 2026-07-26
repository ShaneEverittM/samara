# Exploratory Design: Fluent HTTP Response Handling

Status: the narrow HTTP continuation-builder recommendation was accepted on
July 25, 2026 and is specified by ADR-0005. Its API now follows Samara's paired
continuation convention: `into_command(&http_capability)` uses
`Message: From<BoundaryValue>`, while
`into_command_with(&http_capability, mapper)` accepts an explicit mapper.
ADR-0008 added the capability argument without changing the pure HTTP pipeline
semantics. The general `EffectPlan` sketches in this note remain exploratory
and are not architecture contracts.

## Decision resolution

Samara initially treats status policy and JSON decoding as deterministic pure
mapper behavior. The runtime traces the raw terminal `HttpRequest` outcome;
the Component still receives explicit typed status or decoding failure through
its mapped Message. More specific diagnostics may later be emitted through a
separate observability design, but this slice adds no tracing behavior.

A general `EffectPlan`, composed outer-outcome trace, and user-defined Effect
Layer API are deferred until concrete evidence requires them.

## Recommendation

The accepted slice uses a narrow, typed `HttpResponsePipeline` that compiles to
the existing raw `HttpRequest` EffectDescriptor and its existing pure one-shot
Command mapper. Do **not** introduce a general runtime-owned `EffectPlan` yet.

With a canonical
`From<EffectOutcome<TimeResponse, HttpResponseError>> for Message` conversion,
the intended default spelling is:

```rust
HttpRequest::get("https://api.coinbase.com/v2/time")
    .on_response()
    .require_success()
    .json::<TimeResponse>()
    .into_command(&self.http)
```

When the same outcome type has call-site-specific meaning or the continuation
must capture domain context, the explicit spelling is
`into_command_with(&self.http, mapper)`. Both forms build the same Command representation;
the difference is only how the final pure conversion to Message is supplied.

`on_response()` is the explicit request-to-response phase boundary. It consumes
the `HttpRequest` and returns a different type. Request modifiers therefore do
not exist after that call, and response operations do not exist before it.

This is both a reader-visible and compiler-enforced distinction:

```rust
HttpRequest::post(url)
    .with_header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
    .with_body(encoded) // request construction
    .on_response()      // consumes HttpRequest
    .require_success()  // response status policy
    .json::<Reply>()     // response body decoding
```

A future request JSON convenience should be named `with_json_body(&value)` and
return a `Result`, while response JSON decoding remains `json::<T>()`. The type
boundary already makes the two unambiguous; the asymmetric names make the
distinction obvious even when reading one line in isolation.

## Why `on_response()`

| Boundary spelling | Assessment |
| --- | --- |
| `on_response()` | Recommended. It visibly changes the subject of the following chain. Its slight callback connotation is acceptable because the returned type and following methods make the pipeline evident. |
| `response()` | Shorter, but can read as though it obtains or executes a response during a pure transition. |
| `handle_response()` | Clear about the phase, but conventionally suggests an immediate closure argument. |
| `response_body()` | Too narrow because status and headers are response concerns before body decoding. |

Request-building methods should consistently use `with_*`: `with_header`,
`with_body`, and eventually `with_json_body`. Response operations should name
the policy or transformation directly: `require_success`, `json`, and
potentially later `text` or `require_header`.

## Accepted public shape

The public type can hide a boxed one-shot transformer so users see only the
meaningful output and error types:

```rust
#[must_use = "an HTTP response pipeline is inert until converted into a Command"]
pub struct HttpResponsePipeline<Output, ResponseError> {
    request: HttpRequest,
    transform: Box<
        dyn FnOnce(
                EffectOutcome<HttpResponse, HttpError>,
            ) -> EffectOutcome<Output, ResponseError>
            + Send
            + 'static,
    >,
}

impl HttpRequest {
    pub fn on_response(self) -> HttpResponsePipeline<HttpResponse, HttpError>;
}

impl HttpResponsePipeline<HttpResponse, HttpError> {
    pub fn require_success(
        self,
    ) -> HttpResponsePipeline<HttpResponse, HttpResponseError>;

    // This does not imply require_success().
    pub fn json<T>(self) -> HttpResponsePipeline<T, HttpResponseError>
    where
        T: serde::de::DeserializeOwned + Send + 'static;
}

impl HttpResponsePipeline<HttpResponse, HttpResponseError> {
    pub fn json<T>(self) -> HttpResponsePipeline<T, HttpResponseError>
    where
        T: serde::de::DeserializeOwned + Send + 'static;
}

impl<Output, ResponseError> HttpResponsePipeline<Output, ResponseError>
where
    Output: Send + 'static,
    ResponseError: Send + 'static,
{
    pub fn into_command<Message>(
        self,
        capability: &EffectCapability<HttpRequest>,
    ) -> Command<Message>
    where
        Message: From<EffectOutcome<Output, ResponseError>> + Send + 'static;

    pub fn into_command_with<Message, Map>(
        self,
        capability: &EffectCapability<HttpRequest>,
        map: Map,
    ) -> Command<Message>
    where
        Message: Send + 'static,
        Map: FnOnce(EffectOutcome<Output, ResponseError>) -> Message
            + Send
            + 'static;
}
```

The type should be public because it appears in public method signatures, but it
need not be placed in the prelude initially. Most users should construct and
consume it without naming it. Like `Command`, it should be non-`Clone` and need
not promise `Debug` or equality: it owns one-shot pure continuation state.

`require_success()` is only available while the pipeline still carries a raw
`HttpResponse`. After `json::<T>()`, the status is no longer available, so this
does not compile:

```compile_fail
HttpRequest::get(url)
    .on_response()
    .json::<Reply>()
    .require_success();
```

This makes operation order explicit instead of silently retaining hidden
response metadata.

## The final value and complete transition shapes

The final fluent value remains inert. `into_command` and `into_command_with`
are the explicit points at which it becomes finite work for the runtime. The
default form uses the standard `From` conversion; the `_with` form preserves an
explicit mapper. This API distinction adds no runtime lookup or new execution
semantics.

### Keep the complete typed outcome in one Message

```rust
enum Message {
    GetTime,
    TimeRequestFinished(
        EffectOutcome<TimeResponse, HttpResponseError>,
    ),
}

impl From<EffectOutcome<TimeResponse, HttpResponseError>> for Message {
    fn from(outcome: EffectOutcome<TimeResponse, HttpResponseError>) -> Self {
        Self::TimeRequestFinished(outcome)
    }
}

fn update(&self, model: &mut Model, message: Message) -> Command<Message> {
    match message {
        Message::GetTime => {
            HttpRequest::get("https://api.coinbase.com/v2/time")
                .on_response()
                .require_success()
                .json::<TimeResponse>()
                .into_command(&self.http)
        }
        Message::TimeRequestFinished(EffectOutcome::Succeeded(response)) => {
            model.current_time = Some(response.data.iso);
            samara::println!(&self.stdout, "Got time: {}", response.data.iso)
        }
        Message::TimeRequestFinished(EffectOutcome::Failed(error)) => {
            samara::eprintln!(&self.stderr, "Failed to get time: {error}")
        }
        Message::TimeRequestFinished(EffectOutcome::Cancelled(reason)) => {
            samara::eprintln!(&self.stderr, "Time request was cancelled: {reason:?}")
        }
    }
}
```

### Map immediately into domain-specific Messages

```rust
match message {
    Message::GetTime => {
        HttpRequest::get(TIME_URL)
            .on_response()
            .require_success()
            .json::<TimeResponse>()
            .into_command_with(&self.http, |outcome| match outcome {
                EffectOutcome::Succeeded(response) => {
                    Message::TimeRetrieved(response.data.iso)
                }
                EffectOutcome::Failed(error) => {
                    Message::FailedToGetTime(error.into())
                }
                EffectOutcome::Cancelled(reason) => {
                    Message::FailedToGetTime(GetTimeError::Cancelled(reason))
                }
            })
    }
    Message::TimeRetrieved(time) => {
        model.current_time = Some(time);
        samara::println!(&self.stdout, "Got time: {time}")
    }
    Message::FailedToGetTime(error) => {
        samara::eprintln!(&self.stderr, "Failed to get time: {error}")
    }
}
```

### Decode a useful non-success response deliberately

`json()` by itself decodes any HTTP status. It does not silently insert a 2xx
policy. Assuming the decoded outcome has a canonical `From` conversion into
`Message`:

```rust
match message {
    Message::FetchRejection => {
        HttpRequest::get(REJECTION_URL)
            .on_response()
            .json::<ApiRejection>()
            .into_command(&self.http)
    }
    Message::RejectionDecoded(outcome) => {
        // A valid JSON body from a 400 response can be Succeeded here.
        record_rejection(model, outcome);
        Command::none()
    }
}
```

### Configure a raw request before response handling

Assuming the raw response-pipeline outcome has a canonical `From` conversion
into `Message`:

```rust
match message {
    Message::Submit(encoded) => {
        HttpRequest::new(http::Method::POST, SUBMIT_URL)
            .with_header(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("application/json"),
            )
            .with_body(encoded)
            .on_response()
            .require_success()
            .into_command(&self.http)
    }
    Message::SubmitFinished(outcome) => {
        apply_submit_outcome(model, outcome);
        Command::none()
    }
}
```

## Error algebra and transformation order

The raw terminal boundary remains exactly the accepted ADR-0005 contract:

```text
EffectOutcome<HttpResponse, HttpError>
```

`HttpError` continues to mean only request/client configuration or transport
failure. Cancellation remains the separate `EffectOutcome::Cancelled` terminal
condition and is never folded into an error enum.

Response policy can add a separate error algebra:

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

The operations occur in declaration order:

1. `HttpError` ends the pipeline as `HttpResponseError::Http`.
2. Cancellation passes through unchanged and no response operation runs.
3. `require_success()` accepts 2xx and turns any other status into
   `HttpResponseError::Status`; later decoding does not run for that response.
4. `json::<T>()` decodes the body only if the outcome has reached it. Without a
   preceding `require_success()`, every HTTP status is eligible for decoding.
5. A JSON failure becomes `HttpResponseError::Json` and retains the raw
   response.

This preserves the distinction among transport failure, selected application
status policy, decoding failure, and cancellation.

## Accepted lowering

The paired methods are approximately:

```rust
pub fn into_command<Message>(
    self,
    capability: &EffectCapability<HttpRequest>,
) -> Command<Message>
where
    Message: From<EffectOutcome<Output, ResponseError>> + Send + 'static,
{
    self.into_command_with(capability, Message::from)
}

pub fn into_command_with<Message, Map>(
    self,
    capability: &EffectCapability<HttpRequest>,
    map: Map,
) -> Command<Message>
where
    Message: Send + 'static,
    Map: FnOnce(EffectOutcome<Output, ResponseError>) -> Message
        + Send
        + 'static,
{
    let Self { request, transform } = self;
    Command::effect_with(capability, request, move |raw_outcome| {
        map(transform(raw_outcome))
    })
}
```

Consequently:

- live execution still binds only `HttpRequest` through `bind_http()`;
- controlled execution still uses `control_effect::<HttpRequest>()` and
  `next_effect::<HttpRequest>()`;
- the controlled harness supplies a raw `HttpResponse` or `HttpError`;
- the identical pure status/JSON transformation runs in the existing Command
  mapper in both profiles;
- no descriptor is cloned and every transformer and application mapper runs at
  most once; and
- request inspection, missing-binding diagnostics, Drain, Cancel, and
  structured ownership remain unchanged.

This is profile-independent response handling, but mechanically it is an HTTP
continuation builder rather than a new runtime-owned architectural Layer. The
distinction should remain explicit in documentation until a general
`EffectPlan` exists.

## Narrow builder versus a general EffectPlan

| Concern | HTTP continuation builder | Runtime-owned EffectPlan |
| --- | --- | --- |
| Proposed fluent spelling | Yes | Yes |
| Controlled terminal remains `HttpRequest` | Yes | Yes |
| Existing runtime/trace changes | None | Substantial |
| Status/JSON outcome visible as a runtime semantic event | No; it is pure mapper behavior | Yes |
| Reusable for non-HTTP effects | No | Yes |
| General type-erasure and chain validation | Avoided | Required |
| Delivery risk for the next slice | Low | High |
| Can preserve this surface syntax later | Yes | Yes |

The narrow builder is recommended because the concrete user pain is HTTP
response ceremony, while the current contracts deliberately leave the general
Rust shape of Effect Layers unfrozen. It proves the desired syntax and error
semantics without prematurely changing every effect's lowering, inspection,
trace, and binding path.

Its principal limitation is observability: the generic trace records the raw
terminal `HttpRequest` outcome. A 200 response that later fails JSON decoding
is therefore a terminal effect success followed by pure mapper behavior, not a
traced effect failure. The Component still receives the correct explicit
Message. If this distinction is unacceptable, the general EffectPlan is not an
optional refactor; it is the semantic feature required before shipping the
fluent API.

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

The terminal descriptor is moved—not cloned—into the selected Driver or
`PendingEffect<HttpRequest>`. The runtime retains the ordered layers and the
application's optional one-shot Message mapper under the effect occurrence.
Completing the terminal descriptor consumes each layer exactly once, then
consumes the Message mapper at most once.

Cancellation should bypass `map_result` and remain
`EffectOutcome::Cancelled(reason)` at every layer. This makes it impossible for
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

## Conformance evidence for the accepted slice

The implementation should be test-first around these observable claims:

1. `on_response()` is pure, consumes the request, and a compile-fail example
   proves request modifiers are unavailable afterward.
2. `into_command` and `into_command_with` still expose the exact raw request
   through `next_effect::<HttpRequest>()`; no new controlled or live binding is
   needed.
3. A 2xx valid JSON response produces `Succeeded(T)` in controlled execution.
4. A non-2xx valid JSON response succeeds under `json::<T>()` alone.
5. The same response fails with `Status` under
   `require_success().json::<T>()`, and JSON decoding does not run.
6. A 2xx invalid body fails with `Json` and retains the raw response.
7. Configuration/transport `HttpError` and `Cancelled` pass through their
   distinct channels.
8. The default `From` continuation, an explicit `_with` mapper, and each
   response operation execute at most once; equivalent conversions produce
   equivalent Messages.
9. A local live response and an equivalent controlled fixture produce the same
   Component Message and model state after the raw boundary.
10. Existing raw HTTP, generic Effect, discarded-outcome, Drain, Cancel, and
    trace tests remain unchanged and green.

## Ergonomics and public dependency boundaries

- `json::<T>()` needs `T: DeserializeOwned + Send + 'static`: the body is fully
  owned, and the value crosses Samara's sendable outcome/Message boundary.
- The pipeline and all closures must be `Send + 'static`; `Sync` is unnecessary
  for one finite invocation.
- Boxing one small one-shot transform is an acceptable first-cut cost. Avoid
  exposing deeply nested generic closure types merely to remove this
  allocation before profiling shows it matters.
- Keep `HttpResponsePipeline` out of the prelude initially; export the error
  types there only if ordinary Message/error matching repeatedly needs them.
- Do not re-export `http`, `serde`, or `serde_json`. Samara's existing raw HTTP
  API already uses ecosystem `http` types by their canonical paths, and users
  deriving response types already depend directly on serde. Re-exporting would
  create a second spelling and couple Samara's namespace to ecosystem versions.
- Adding response JSON moves serde/serde_json into Samara's normal dependency
  surface. Feature-gating can be reconsidered when Samara has a broader feature
  policy; one isolated feature for the first common response decoder would add
  onboarding ceremony prematurely.

## Fit with the vision

This design directly expresses four separate intentions in their source order:
construct the request, cross into response handling, select status policy, and
decode the body. It is readable without forcing terse shorthand. It is
ergonomic, but none of the steps execute work or invent hidden network policy.

Mechanism and policy remain separated:

- the terminal Driver performs raw HTTP transport;
- the existing runtime owns scheduling, cancellation, and delivery;
- the pure pipeline applies explicitly selected response policy; and
- the Component decides which resulting Message changes its Model.

In particular, `json()` does not smuggle in status policy, redirect behavior,
retry, timeout, content-type enforcement, or error logging.

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

## Resolved load-bearing question

Must a status/JSON transformation failure be a first-class **runtime-traced
Effect outcome**, or is it sufficient for the same deterministic pure mapper to
turn the raw terminal outcome into an explicit Component Message in both live
and controlled execution?

Decision: treat it as pure mapper behavior. This matches ADR-0005's terminal
boundary, solves the demonstrated ceremony, and preserves the proposed source
syntax. If later evidence requires a runtime-traced composed outcome, write an
EffectPlan ADR and conformance contract before changing these semantics.
