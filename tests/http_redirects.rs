//! ADR-0012: redirects are pure policy over individually visible HTTP effects.
use std::{convert::Infallible, time::Duration};

use http::{HeaderMap, HeaderValue, Method, Version};
use samara::prelude::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

type Outcome = EffectOutcome<HttpResponse, HttpError>;
struct Observed(Outcome);
impl EffectDescriptor for Observed {
    type Output = ();
    type Error = Infallible;
}
struct Probe {
    http: EffectCapability<HttpRequest>,
    observed: EffectCapability<Observed>,
    request: HttpRequest,
    limit: usize,
}
impl Component for Probe {
    type Model = Vec<Outcome>;
    type Message = Outcome;
    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::default().with_command(
            self.request
                .clone()
                .on_response()
                .follow_redirects(self.limit)
                .into_command_with(&self.http, |outcome| outcome),
        )
    }
    fn update(&self, model: &mut Self::Model, outcome: Outcome) -> Command<Outcome> {
        model.push(outcome.clone());
        Command::effect_discarding_outcome(&self.observed, Observed(outcome))
    }
}
fn program(request: HttpRequest, limit: usize) -> (Program, ComponentRef<Probe>) {
    let mut builder = Program::builder();
    let http = builder.effect();
    let observed = builder.effect();
    let probe = builder.component(
        ComponentId::new("probe"),
        Probe {
            http,
            observed,
            request,
            limit,
        },
    );
    (builder.build().unwrap(), probe)
}
fn controlled(request: HttpRequest, limit: usize) -> (ControlledRuntime, ComponentRef<Probe>) {
    let (program, probe) = program(request, limit);
    let mut runtime = ControlledRuntime::builder(program)
        .control_effect::<HttpRequest>()
        .control_effect::<Observed>()
        .build()
        .unwrap();
    runtime.run_until_idle().unwrap();
    (runtime, probe)
}
fn response(status: u16, location: Option<&str>) -> HttpResponse {
    let mut headers = HeaderMap::new();
    if let Some(location) = location {
        headers.insert("location", HeaderValue::from_str(location).unwrap());
    }
    HttpResponse::new(
        StatusCode::from_u16(status).unwrap(),
        Version::HTTP_11,
        headers,
        "body",
    )
}
fn finish(runtime: &mut ControlledRuntime, response: HttpResponse) {
    let pending = runtime.next_effect::<HttpRequest>().unwrap();
    runtime
        .complete(pending, EffectOutcome::Succeeded(response))
        .unwrap();
    runtime.run_until_idle().unwrap();
}
fn redirect_error(runtime: &ControlledRuntime, probe: &ComponentRef<Probe>) {
    let model = runtime.state(probe).unwrap();
    assert_eq!(model.len(), 1);
    assert!(
        matches!(&model[0], EffectOutcome::Failed(error) if error.kind() == HttpErrorKind::Redirect)
    );
}

#[test]
fn relative_hops_are_visible_causal_and_deliver_only_one_final_message() {
    let (mut runtime, probe) = controlled(HttpRequest::get("https://example.test/a/start"), 3);
    let first = runtime.next_effect::<HttpRequest>().unwrap();
    assert_eq!(first.intent.url(), "https://example.test/a/start");
    runtime
        .complete(
            first,
            EffectOutcome::Succeeded(response(301, Some("../next#fragment"))),
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.state(&probe).unwrap().is_empty());
    let second = runtime.next_effect::<HttpRequest>().unwrap();
    assert_eq!(second.intent.url(), "https://example.test/next");
    runtime
        .complete(
            second,
            EffectOutcome::Succeeded(response(302, Some("/done"))),
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.state(&probe).unwrap().is_empty());
    finish(&mut runtime, response(200, None));
    assert!(
        matches!(&runtime.state(&probe).unwrap()[0], EffectOutcome::Succeeded(r) if r.status() == StatusCode::OK)
    );
    let outcomes = runtime
        .trace()
        .iter()
        .filter(|r| matches!(r.event, TraceEvent::EffectOutcome { .. }))
        .count();
    assert_eq!(outcomes, 3);
    assert!(runtime.cancel().unwrap().is_clean());
}

#[test]
fn zero_and_finite_limits_stop_before_an_extra_request() {
    for limit in [0, 1, 3] {
        let (mut runtime, probe) = controlled(HttpRequest::get("https://example.test/loop"), limit);
        for _ in 0..=limit {
            finish(&mut runtime, response(301, Some("/loop")));
        }
        redirect_error(&runtime, &probe);
        assert!(runtime.next_effect::<HttpRequest>().is_err());
        assert!(runtime.cancel().unwrap().is_clean());
    }
}

#[test]
fn missing_location_and_non_redirect_statuses_are_final_responses() {
    for (status, location) in [(301, None), (304, Some("/other")), (404, None), (200, None)] {
        let (mut runtime, probe) = controlled(HttpRequest::get("https://example.test/"), 5);
        finish(&mut runtime, response(status, location));
        assert!(
            matches!(&runtime.state(&probe).unwrap()[0], EffectOutcome::Succeeded(r) if r.status().as_u16() == status)
        );
        assert!(runtime.next_effect::<HttpRequest>().is_err());
        runtime.cancel().unwrap();
    }
}

#[test]
fn malformed_unsupported_credentialed_and_downgrade_targets_fail() {
    for location in [
        "http://example.test/",
        "ftp://example.test/",
        "https://user:secret@example.test/",
        "http://[",
    ] {
        let (mut runtime, probe) = controlled(HttpRequest::get("https://example.test/"), 5);
        finish(&mut runtime, response(302, Some(location)));
        redirect_error(&runtime, &probe);
        assert!(runtime.next_effect::<HttpRequest>().is_err());
        runtime.cancel().unwrap();
    }
    let (mut runtime, probe) = controlled(HttpRequest::get("https://example.test/"), 5);
    let mut headers = HeaderMap::new();
    headers.append("location", HeaderValue::from_static("/one"));
    headers.append("location", HeaderValue::from_static("/two"));
    finish(
        &mut runtime,
        HttpResponse::new(StatusCode::FOUND, Version::HTTP_11, headers, ""),
    );
    redirect_error(&runtime, &probe);
    runtime.cancel().unwrap();
}

#[test]
fn redirect_method_and_body_rules_are_explicit() {
    for status in [301, 302, 303, 307, 308] {
        for method in [Method::GET, Method::HEAD, Method::POST, Method::PUT] {
            let request = HttpRequest::new(method.clone(), "https://example.test/")
                .with_body("payload")
                .with_header(
                    http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/plain"),
                )
                .with_header(http::header::CONTENT_LENGTH, HeaderValue::from_static("7"))
                .with_header(
                    http::header::EXPECT,
                    HeaderValue::from_static("100-continue"),
                );
            let (mut runtime, _) = controlled(request, 1);
            finish(&mut runtime, response(status, Some("/next")));
            let hop = runtime.next_effect::<HttpRequest>().unwrap().intent;
            let drops_body =
                status == 303 || ([301, 302].contains(&status) && method == Method::POST);
            let expected = if drops_body && method != Method::HEAD {
                Method::GET
            } else {
                method.clone()
            };
            assert_eq!(hop.method(), expected, "status {status}, method {method}");
            assert_eq!(hop.body().is_empty(), drops_body);
            assert_eq!(hop.headers().contains_key("content-type"), !drops_body);
            assert_eq!(hop.headers().contains_key("content-length"), !drops_body);
            assert_eq!(hop.headers().contains_key("expect"), !drops_body);
            runtime.cancel().unwrap();
        }
    }
}

#[test]
fn origin_changes_strip_credentials_and_all_hops_remove_routing_headers() {
    for target in [
        "https://example.test/next",
        "https://other.test/",
        "https://example.test:444/",
    ] {
        let mut request = HttpRequest::get("https://example.test/");
        for name in [
            "authorization",
            "cookie",
            "cookie2",
            "host",
            "referer",
            "proxy-authorization",
            "x-public",
        ] {
            request
                .headers_mut()
                .insert(name, HeaderValue::from_static("value"));
        }
        let mut secret = HeaderValue::from_static("secret");
        secret.set_sensitive(true);
        request.headers_mut().insert("x-secret", secret);
        let (mut runtime, _) = controlled(request, 2);
        finish(&mut runtime, response(307, Some(target)));
        let hop = runtime.next_effect::<HttpRequest>().unwrap().intent;
        for name in ["host", "referer", "proxy-authorization"] {
            assert!(!hop.headers().contains_key(name));
        }
        for name in ["authorization", "cookie", "cookie2", "x-secret"] {
            assert_eq!(
                hop.headers().contains_key(name),
                target == "https://example.test/next"
            );
        }
        assert!(hop.headers().contains_key("x-public"));
        runtime.cancel().unwrap();
    }
}

#[test]
fn transport_failure_and_explicit_cancellation_end_the_chain() {
    for outcome in [
        EffectOutcome::Failed(HttpError::new(HttpErrorKind::Transport, "offline")),
        EffectOutcome::Cancelled(CancelReason::Superseded),
    ] {
        let (mut runtime, probe) = controlled(HttpRequest::get("https://example.test/"), 3);
        finish(&mut runtime, response(302, Some("/next")));
        let pending = runtime.next_effect::<HttpRequest>().unwrap();
        runtime.complete(pending, outcome.clone()).unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(runtime.state(&probe).unwrap(), &vec![outcome]);
        assert!(runtime.next_effect::<HttpRequest>().is_err());
        runtime.cancel().unwrap();
    }
}

struct ObserveDriver(tokio::sync::mpsc::UnboundedSender<Outcome>);
impl EffectDriver<Observed> for ObserveDriver {
    fn execute(&self, value: Observed) -> BoxFuture<Result<(), Infallible>> {
        let sender = self.0.clone();
        Box::pin(async move {
            sender.send(value.0).unwrap();
            Ok(())
        })
    }
}

#[tokio::test]
async fn live_drain_follows_relative_redirect_through_the_same_driver() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/start", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            for (path, reply) in [
                ("/start", "HTTP/1.1 301 Moved Permanently\r\nLocation: /done\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
                ("/done", "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = socket.read(&mut buffer).await.unwrap();
                    assert_ne!(n, 0);
                    request.extend_from_slice(&buffer[..n]);
                }
                assert!(String::from_utf8(request).unwrap().starts_with(&format!("GET {path} HTTP/1.1")));
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let (program, _) = program(HttpRequest::get(url), 3);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let task = LiveRuntime::builder(program).bind_http().bind_effect::<Observed, _>(ObserveDriver(tx)).build().unwrap().spawn();
        assert!(task.shutdown(Shutdown::Drain).await.unwrap().is_clean());
        assert!(matches!(rx.recv().await.unwrap(), EffectOutcome::Succeeded(r) if r.status() == StatusCode::OK));
        assert!(rx.try_recv().is_err());
        server.await.unwrap();
    }).await.unwrap();
}

#[test]
fn controlled_traces_repeat_and_next_hop_is_caused_by_previous_outcome() {
    fn run() -> Vec<TraceRecord> {
        let (mut runtime, _) = controlled(HttpRequest::get("https://example.test/"), 2);
        finish(&mut runtime, response(301, Some("/next")));
        let trace = runtime.trace();
        let raw_outcome = trace
            .iter()
            .find(|r| matches!(r.event, TraceEvent::EffectOutcome { .. }))
            .unwrap();
        assert!(trace.iter().any(|r| r.cause == Some(raw_outcome.id)
            && matches!(
                r.event,
                TraceEvent::CommandEmitted {
                    kind: TraceCommandKind::Effect,
                    ..
                }
            )));
        finish(&mut runtime, response(200, None));
        let trace = runtime.trace().to_vec();
        runtime.cancel().unwrap();
        trace
    }
    assert_eq!(run(), run());
}

struct JsonProbe(EffectCapability<HttpRequest>);
impl Component for JsonProbe {
    type Model = Option<EffectOutcome<serde_json::Value, HttpResponseError>>;
    type Message = EffectOutcome<serde_json::Value, HttpResponseError>;
    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::default().with_command(
            HttpRequest::get("https://example.test/")
                .on_response()
                .follow_redirects(0)
                .follow_redirects(1)
                .require_success()
                .json::<serde_json::Value>()
                .into_command_with(&self.0, |outcome| outcome),
        )
    }
    fn update(&self, model: &mut Self::Model, outcome: Self::Message) -> Command<Self::Message> {
        *model = Some(outcome);
        Command::none()
    }
}

#[test]
fn status_and_json_transforms_run_only_after_following_and_keep_typed_errors() {
    for status in [200, 404, 301] {
        let mut builder = Program::builder();
        let http = builder.effect();
        let probe = builder.component(ComponentId::new("json"), JsonProbe(http));
        let mut runtime = ControlledRuntime::builder(builder.build().unwrap())
            .control_effect::<HttpRequest>()
            .build()
            .unwrap();
        runtime.run_until_idle().unwrap();
        finish(&mut runtime, response(301, Some("/next")));
        assert!(runtime.state(&probe).unwrap().is_none());
        let mut headers = HeaderMap::new();
        headers.insert("location", HeaderValue::from_static("/again"));
        finish(
            &mut runtime,
            HttpResponse::new(
                StatusCode::from_u16(status).unwrap(),
                Version::HTTP_11,
                headers,
                r#"{"value":42}"#,
            ),
        );
        match (status, runtime.state(&probe).unwrap().as_ref().unwrap()) {
            (200, EffectOutcome::Succeeded(value)) => assert_eq!(value["value"], 42),
            (404, EffectOutcome::Failed(HttpResponseError::Status(error))) => {
                assert_eq!(error.response().status(), StatusCode::NOT_FOUND)
            }
            (301, EffectOutcome::Failed(HttpResponseError::Http(error))) => {
                assert_eq!(error.kind(), HttpErrorKind::Redirect)
            }
            _ => panic!("unexpected pipeline outcome"),
        }
        assert!(runtime.next_effect::<HttpRequest>().is_err());
        runtime.cancel().unwrap();
    }
}

#[tokio::test]
async fn live_cancel_aborts_active_redirect_hop_without_application_completion() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/start", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            for index in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = socket.read(&mut buffer).await.unwrap();
                    assert_ne!(n, 0);
                    request.extend_from_slice(&buffer[..n]);
                }
                if index == 1 { return socket; }
                socket.write_all(b"HTTP/1.1 302 Found\r\nLocation: /pending\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
            }
            unreachable!()
        });
        let (program, _) = program(HttpRequest::get(url), 3);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let task = LiveRuntime::builder(program).bind_http().bind_effect::<Observed, _>(ObserveDriver(tx)).build().unwrap().spawn();
        let mut socket = server.await.unwrap();
        assert!(task.shutdown(Shutdown::Cancel).await.unwrap().is_clean());
        assert!(rx.try_recv().is_err());
        let mut buffer = [0; 16];
        assert_eq!(socket.read(&mut buffer).await.unwrap(), 0);
    }).await.unwrap();
}
