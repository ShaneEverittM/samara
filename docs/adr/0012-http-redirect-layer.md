# ADR 0012: Explicit HTTP Redirect Layer

- Status: Accepted for implementation
- Date: September 24, 2026
- Extends: [ADR-0005](0005-first-party-http-effect.md)

## Context and decision

The URL checker needs opt-in redirect following on the response pipeline.
`on_response().follow_redirects(max_hops)` declares a finite HTTP Layer before
`require_success()` or `json()`. No opt-in means exactly the existing behavior.
The limit counts additional requests, not the initial request; zero rejects a
followable redirect. Repeating the builder method replaces the limit.

Each hop is an ordinary `HttpRequest` with its own terminal outcome, capability
validation, work accounting, and controlled trace. A private pure continuation
can return the next Command instead of an application Message. Both runtime
profiles interpret that continuation with the same originating Component and
causal relationship. The mechanism knows nothing about HTTP. Intermediate hops
do not mutate the Model or invoke the application mapper; the final outcome
passes through the response transforms and one application mapper. The Layer's
owned request context is finite operation state, not mutable application state.
A public general Effect-Layer API remains deferred.

### Alternatives

| Approach | Consequence | Decision |
| --- | --- | --- |
| Enable reqwest redirects | Hides hops and policy from controlled execution | Reject |
| Require every app to implement redirect Messages | Explicit but duplicates protocol policy | Keep available, add reusable Layer |
| Pure Layer over explicit terminal requests | Same policy and hop visibility in both profiles | Choose |

## Redirect contract

Following [RFC 9110 section 15.4](https://www.rfc-editor.org/rfc/rfc9110.html#section-15.4):

- Follow 301, 302, 303, 307, and 308 when exactly one `Location` is present.
  Missing Location or other statuses terminate with the raw response.
- Resolve relative Location against the current request URL; remove fragments.
  Invalid targets, multiple Locations, URL credentials, non-HTTP(S) schemes,
  and HTTPS-to-HTTP downgrades fail with `HttpErrorKind::Redirect`.
- 301/302 change POST to GET. 303 changes non-HEAD methods to GET. When dropping
  the body (including 303 HEAD), remove content headers, transfer framing, and
  Expect. Other methods on 301/302 and all methods on 307/308 preserve the body.
- Remove Host, Referer, and Proxy-Authorization on every hop. On an origin change
  (scheme, host, or effective port), also remove Authorization, Cookie, Cookie2,
  and headers marked sensitive. Applications remain responsible for marking
  custom credential headers sensitive. No Referer or cookies are synthesized.
- Exhausting the limit fails with `HttpErrorKind::Redirect` before issuing
  another request. Cycles terminate by the same finite bound.
- Raw HTTP failures and explicit effect cancellation terminate the Layer. Status
  and JSON transforms see only the final response. Layer errors use HttpError's
  typed Redirect kind; they do not relabel a raw hop's successful trace outcome.

## Invariants, failures, and lifecycle

Model/Message ownership, per-Component serialization, and single-hop Driver
semantics remain unchanged. No async closure or ambient I/O is introduced.
Controlled completion deterministically interprets the next finite intent;
its Command trace is a child of the preceding raw outcome. Ordinary direct
mappers still enqueue Messages as before. Direct `into_effect()` inspection is
only supported for effects with a direct Message mapper; use controlled hops
for a layered pipeline, or `effect_intent()` for its first request.

Drain follows the bounded chain and waits for the final Message and consequent
work. Cancel aborts the active hop and drops the continuation without invoking
the application mapper. The hop bound is not a timeout or body-size limit; a
hung transport can still hold Drain open. No new automatic retries or deadlines.

## Acceptance evidence

`tests/http_redirects.rs` covers initial intent, per-hop controlled visibility,
relative URLs, terminal mapping, status/JSON composition, finite limits, URL and
header policy, method/body rules, failure/cancellation, deterministic traces,
and live redirect execution with Drain/Cancel. Existing HTTP and pipeline tests
must continue to prove unchanged default behavior.

## Rollback

Remove `.follow_redirects(...)` at call sites to restore raw response handling.
The terminal Driver and its binding are unchanged. Removing the new builder
method and private continuation path restores the old implementation without
changing existing application Models or stored data.
