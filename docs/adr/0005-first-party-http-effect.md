# ADR 0005: First-Party HTTP Effect

- Status: Accepted
- Date: July 24, 2026
- Amended: July 25, 2026 (pure fluent response pipeline and paired
  continuation API)
- Decision owners: Samara maintainers
- Extends: [ADR 0004](0004-initial-live-runtime-semantics.md)

> Capability amendment (July 25, 2026):
> [ADR-0008](0008-closed-program-capabilities.md) requires an
> `EffectCapability<HttpRequest>` when this pipeline lowers into a Command.
> The raw HTTP boundary and pure response semantics below are unchanged.

## Context

The first real-application experiment had to define a private EffectDescriptor,
Driver, client, error wrapper, and binding to perform one ordinary HTTP request.
The explicit effect boundary was valuable, but the generic protocol plumbing was
incidental to the application. A first-party finite HTTP boundary can remove
that plumbing without teaching the runtime JSON, status, retry, redirect, or
other application policy.

The underlying HTTP client has behavior of its own. In particular, reqwest
follows redirects and retries some protocol failures by default. Inheriting
those defaults would make one declared request produce undeclared additional
requests and would hide an HTTP response from application logic.

## Decision

### The descriptor and response are raw owned data

`HttpRequest` is one finite terminal EffectDescriptor. It owns an
`http::Method`, URL text, `http::HeaderMap`, and `bytes::Bytes` body.
Construction performs no I/O. URL validation that depends on the live client is
reported as a typed configuration failure when the Driver realizes the effect.

`HttpResponse` owns the raw response status, HTTP version, headers, and fully
buffered `bytes::Bytes` body. All HTTP statuses, including redirects and 4xx or
5xx responses, are successful descriptor outputs. Interpreting a status or
decoding a body belongs in a pure mapper, Layer, or application transition.

`HttpError` distinguishes configuration failures from transport failures and
retains a diagnostic message. Configuration includes client or request
construction and invalid or unsupported URL configuration. Transport includes
sending the request and receiving its complete response body. Controlled tests
can construct the same typed errors without creating a live client error.

### The first live Driver is pooled and policy-minimal

`LiveRuntimeBuilder::bind_http()` installs one terminal HTTP Driver containing
one reusable reqwest `Client`. The binding and its client live for the runtime
assembly, so sequential invocations can reuse the client's connection pool.
Duplicate HTTP bindings fail through the ordinary duplicate Effect-binding
validation.

The v0 client explicitly:

- does not follow redirects;
- does not retry requests, including reqwest's default protocol retries;
- does not discover or use system proxies;
- does not automatically decompress encoded response bodies; and
- has no Samara-supplied request timeout.

The Driver performs no JSON or text decoding, status classification,
authentication, logging, retry, redirect, or application-specific header policy.
When the descriptor omits `Accept`, it explicitly supplies `Accept: */*` as a
no-preference transport default; an application-supplied `Accept` value wins.
No other application-level header is invented.
It buffers the complete response body without imposing a Samara-specific size
limit. A hung request can therefore make `Shutdown::Drain` wait indefinitely;
`Shutdown::Cancel` still cancels and owns the in-flight Driver future under
ADR-0004.

The transport library may supply protocol-required or conventional wire
defaults such as connection framing. Those mechanics do not alter the inert
descriptor or constitute application decoding or status policy.

### Controlled execution remains the same typed boundary

Controlled execution uses `control_effect::<HttpRequest>()`. It intercepts the
descriptor and supplies an `EffectOutcome<HttpResponse, HttpError>` without
constructing a live client or touching DNS, sockets, TLS, proxies, or the
network. The generic Effect lifecycle continues to govern mapped and
discarded-outcome commands, Drain, Cancel, trace, and work accounting.

### Fluent response handling is pure mapper behavior

`HttpRequest::on_response()` consumes the request and enters a distinct,
must-use response-pipeline type. Request construction remains on
`HttpRequest`, with `with_*` modifiers; response policy and decoding are only
available after this explicit phase boundary. The first accepted operations
are:

- `require_success()`, which accepts only 2xx responses and otherwise produces
  an `HttpStatusError` retaining the complete raw response; and
- `json::<T>()`, which decodes the owned body without implying any status
  policy and produces an `HttpJsonError` retaining both the complete raw
  response and the `serde_json` source error.

A future request-side JSON encoder is reserved for a name such as
`with_json_body`; response-side `json::<T>()` remains unambiguously decoding.

The resulting `HttpResponsePipeline` becomes work only through one of two
paired APIs. `into_command(&http_capability)` uses the canonical standard
conversion `Message: From<EffectOutcome<Output, ResponseError>>`;
`into_command_with(&http_capability, mapper)` accepts an explicit pure mapper
when a call site must capture context or assign a different meaning to the same
outcome type.
Both lower to exactly one ordinary Effect Command with one composed mapper,
equivalent to `Command::effect_with(&http_capability, HttpRequest, mapper)`. The terminal Driver
and controlled boundary therefore remain the raw `HttpRequest`; status checks
and JSON decoding run once as deterministic, synchronous mapper behavior after
the raw outcome. They do not create additional runtime events or change the
trace's terminal outcome kind. This is an API-spelling convention, not a new
runtime behavior or Command kind.

`HttpResponseError` keeps configuration/transport `HttpError`, selected status
policy, and JSON decoding failures distinct. `EffectOutcome::Cancelled`
continues through the pipeline as cancellation and is never converted into
response-error data. Operations run in declared order, so
`require_success().json::<T>()` rejects a non-2xx response before attempting
decoding, while `json::<T>()` alone may successfully decode a non-2xx response.

## Consequences

### Positive

- Applications directly declare ordinary HTTP intent without writing a
  terminal Driver.
- Live invocations reuse one runtime-owned connection pool.
- Controlled tests observe the same method, URL, headers, and body without
  network access.
- Redirects, error statuses, and response decoding remain visible application
  decisions.
- Common status and JSON intent can be written in source order without moving
  those pure decisions into the terminal Driver or runtime.

### Negative

- The first cut buffers complete request and response bodies and is unsuitable
  for streaming or intentionally unbounded bodies.
- There is no first-party timeout, proxy, redirect, retry, middleware, or
  custom-client configuration surface.
- URL validation may arrive as an EffectOutcome rather than a descriptor
  constructor error.
- Generic traces report the raw HTTP terminal outcome. A later status or JSON
  mapper failure is visible to the Component Message but is not a traced
  Effect failure.

## Required Evidence

- Descriptor tests preserve method, URL, duplicate-capable headers, and owned
  body bytes.
- Controlled execution intercepts `HttpRequest` and can return a constructed
  `HttpResponse` without network access.
- A local live server observes request method, headers, and body and returns raw
  status, headers, and body.
- A declared `Accept` header is preserved and an omitted one receives the
  documented no-preference default.
- Redirect and HTTP error statuses are returned without automatic follow or
  status failure.
- Two sequential requests through one binding can reuse one accepted HTTP/1.1
  connection.
- Duplicate `HttpRequest` bindings fail assembly.
- Existing generic finite-effect evidence continues to prove mapped and
  discarded outcomes, Drain, Cancel, and structured ownership.
- The response phase consumes the request, and request modifiers are absent
  from the resulting type.
- `into_command(&http_capability)` uses the Component Message's canonical
  `From` conversion, while `into_command_with(&http_capability, mapper)`
  supports an explicit call-site mapper; both retain the same one-shot lowering
  and runtime behavior.
- Controlled execution still intercepts the exact raw request when a fluent
  pipeline is lowered.
- Valid JSON succeeds for 2xx and, without `require_success`, non-2xx responses.
- `require_success` rejects non-2xx before JSON decoding.
- Status and JSON errors retain the complete response; JSON errors also retain
  their source error.
- Raw HTTP failure and cancellation retain distinct outcome channels.
- Equivalent live and controlled raw responses produce equivalent Component
  results through the same pure response pipeline.

## Explicitly Unresolved

- streaming request or response bodies;
- response body limits;
- timeout and deadline descriptors or Layers;
- redirect, retry, authentication, cookies, proxy, decompression, and cache
  policy;
- configurable or application-supplied client pools;
- HTTP/2- or HTTP/3-specific guarantees;
- request JSON encoding and text, form, multipart, or streaming response
  handling, plus a typed endpoint abstraction;
- user-defined response transforms and a general Effect Layer or `EffectPlan`
  abstraction; and
- runtime tracing of status-policy or response-decoding outcomes.

## Rollback

The first-party binding can be removed while retaining custom EffectDrivers.
Changing raw status handling, implicit request multiplication, controlled
network isolation, pooled-client ownership, cancellation passthrough, or the
raw-only trace boundary requires a superseding ADR and updated executable
evidence.

## Explicit redirect Layer amendment — September 24, 2026

[ADR-0012](0012-http-redirect-layer.md) adds opt-in pipeline redirect following
as a pure Layer issuing individually visible HTTP effects. The terminal Driver
still performs one request and never follows redirects. Pipelines without this
opt-in retain the single-effect pure-mapper contract above.
