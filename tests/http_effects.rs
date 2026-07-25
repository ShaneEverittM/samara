use std::{convert::Infallible, time::Duration};

use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Version};
use samara::prelude::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

const LIVE_TEST_TIMEOUT: Duration = Duration::from_secs(2);

#[test]
fn http_descriptor_preserves_method_url_headers_and_owned_body() {
    let request = HttpRequest::new(Method::POST, "https://example.test/upload")
        .with_header(
            HeaderName::from_static("x-order"),
            HeaderValue::from_static("first"),
        )
        .with_header(
            HeaderName::from_static("x-order"),
            HeaderValue::from_static("second"),
        )
        .with_body(Bytes::from_static(b"owned body"));

    assert_eq!(request.method(), Method::POST);
    assert_eq!(request.url(), "https://example.test/upload");
    assert_eq!(
        request
            .headers()
            .get_all("x-order")
            .iter()
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert_eq!(request.body(), &Bytes::from_static(b"owned body"));

    let cloned = request.clone();
    drop(request);
    assert_eq!(cloned.body(), &Bytes::from_static(b"owned body"));
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SeenHttp {
    Response(HttpResponse),
    Error(HttpError),
    Cancelled,
}

#[derive(Default)]
struct HttpModel {
    seen: Vec<SeenHttp>,
}

enum HttpMessage {
    Start,
    Finished(EffectOutcome<HttpResponse, HttpError>),
    Observed,
}

struct HttpSequence {
    requests: Vec<HttpRequest>,
}

impl Component for HttpSequence {
    type Model = HttpModel;
    type Message = HttpMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(HttpModel::default())
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            HttpMessage::Start => Command::effect(
                self.requests
                    .first()
                    .expect("nonempty test sequence")
                    .clone(),
                HttpMessage::Finished,
            ),
            HttpMessage::Finished(outcome) => {
                model.seen.push(match outcome {
                    EffectOutcome::Succeeded(response) => SeenHttp::Response(response),
                    EffectOutcome::Failed(error) => SeenHttp::Error(error),
                    EffectOutcome::Cancelled(_) => SeenHttp::Cancelled,
                });
                if let Some(request) = self.requests.get(model.seen.len()) {
                    Command::effect(request.clone(), HttpMessage::Finished)
                } else {
                    Command::effect(ObserveHttp(model.seen.clone()), |_| HttpMessage::Observed)
                }
            }
            HttpMessage::Observed => Command::none(),
        }
    }
}

#[derive(Clone, Debug)]
struct ObserveHttp(Vec<SeenHttp>);

impl EffectDescriptor for ObserveHttp {
    type Output = ();
    type Error = Infallible;
}

struct HttpObserver {
    observations: tokio::sync::mpsc::UnboundedSender<Vec<SeenHttp>>,
}

impl EffectDriver<ObserveHttp> for HttpObserver {
    fn execute(&self, descriptor: ObserveHttp) -> BoxFuture<Result<(), Infallible>> {
        let observations = self.observations.clone();
        Box::pin(async move {
            let _ = observations.send(descriptor.0);
            Ok(())
        })
    }
}

fn http_program(requests: Vec<HttpRequest>) -> (Program, ComponentRef<HttpSequence>) {
    let mut builder = Program::builder();
    let probe = builder.component(ComponentId::new("http-sequence"), HttpSequence { requests });
    (builder.build().expect("valid HTTP program"), probe)
}

#[test]
fn controlled_http_is_an_ordinary_typed_effect_boundary() {
    let request = HttpRequest::new(Method::PATCH, "not a live URL")
        .with_body(Bytes::from_static(b"controlled"));
    let (program, probe) = http_program(vec![request]);
    let mut runtime = ControlledRuntime::builder(program)
        .control_effect::<HttpRequest>()
        .control_effect::<ObserveHttp>()
        .build()
        .expect("valid controlled HTTP behavior");

    runtime.send(&probe, HttpMessage::Start).unwrap();
    runtime.run_until_idle().unwrap();
    let pending = runtime.next_effect::<HttpRequest>().unwrap();
    assert_eq!(pending.intent.method(), Method::PATCH);
    assert_eq!(pending.intent.url(), "not a live URL");
    assert_eq!(pending.intent.body(), &Bytes::from_static(b"controlled"));

    let mut headers = HeaderMap::new();
    headers.insert("x-fixture", HeaderValue::from_static("controlled"));
    let response = HttpResponse::new(
        StatusCode::IM_A_TEAPOT,
        Version::HTTP_11,
        headers,
        Bytes::from_static(b"fixture body"),
    );
    runtime
        .complete(pending, EffectOutcome::Succeeded(response.clone()))
        .unwrap();
    runtime.run_until_idle().unwrap();

    let observation = runtime.next_effect::<ObserveHttp>().unwrap();
    assert_eq!(observation.intent.0, vec![SeenHttp::Response(response)]);
    runtime
        .complete(observation, EffectOutcome::Succeeded(()))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.cancel().unwrap().is_clean());
}

#[derive(Debug)]
struct ServerRequest {
    request_line: String,
    headers: String,
    body: Vec<u8>,
}

#[derive(Debug)]
struct ServerReport {
    accepted_connections: usize,
    requests: Vec<ServerRequest>,
}

async fn read_request(stream: &mut TcpStream) -> std::io::Result<Option<ServerRequest>> {
    let mut raw = Vec::new();
    let header_end = loop {
        if let Some(index) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        let mut chunk = [0_u8; 1024];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return if raw.is_empty() {
                Ok(None)
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "connection ended inside request headers",
                ))
            };
        }
        raw.extend_from_slice(&chunk[..read]);
    };

    let headers = String::from_utf8(raw[..header_end].to_vec()).expect("test request is UTF-8");
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length").then(|| {
                value
                    .trim()
                    .parse::<usize>()
                    .expect("numeric content length")
            })
        })
        .unwrap_or(0);
    while raw.len() < header_end + content_length {
        let mut chunk = [0_u8; 1024];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "connection ended inside request body",
            ));
        }
        raw.extend_from_slice(&chunk[..read]);
    }

    Ok(Some(ServerRequest {
        request_line: headers.lines().next().unwrap().to_owned(),
        headers,
        body: raw[header_end..header_end + content_length].to_vec(),
    }))
}

async fn serve_two_requests(listener: TcpListener) -> std::io::Result<ServerReport> {
    let mut accepted_connections = 0;
    let mut requests = Vec::new();

    while requests.len() < 2 {
        let (mut stream, _) = listener.accept().await?;
        accepted_connections += 1;
        while requests.len() < 2 {
            let Some(request) = read_request(&mut stream).await? else {
                break;
            };
            let path = request
                .request_line
                .split_whitespace()
                .nth(1)
                .expect("request target");
            let response = match path {
                "/status" => concat!(
                    "HTTP/1.1 503 Service Unavailable\r\n",
                    "X-Response: status\r\n",
                    "Content-Length: 11\r\n",
                    "Connection: keep-alive\r\n",
                    "\r\n",
                    "unavailable"
                ),
                "/redirect" => concat!(
                    "HTTP/1.1 302 Found\r\n",
                    "Location: /must-not-follow\r\n",
                    "Content-Length: 8\r\n",
                    "Connection: keep-alive\r\n",
                    "\r\n",
                    "redirect"
                ),
                other => panic!("the HTTP Driver made an undeclared request to {other}"),
            };
            requests.push(request);
            stream.write_all(response.as_bytes()).await?;
            stream.flush().await?;
        }
    }

    Ok(ServerReport {
        accepted_connections,
        requests,
    })
}

#[tokio::test]
async fn live_http_preserves_raw_status_and_reuses_one_client_pool() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    let server = tokio::spawn(serve_two_requests(listener));

    let first = HttpRequest::new(Method::POST, format!("http://{endpoint}/status"))
        .with_header(
            HeaderName::from_static("x-request"),
            HeaderValue::from_static("present"),
        )
        .with_header(
            HeaderName::from_static("x-order"),
            HeaderValue::from_static("first"),
        )
        .with_header(
            HeaderName::from_static("x-order"),
            HeaderValue::from_static("second"),
        )
        .with_header(
            http::header::ACCEPT,
            HeaderValue::from_static("application/vnd.samara"),
        )
        .with_body(Bytes::from_static(b"request body"));
    let second = HttpRequest::get(format!("http://{endpoint}/redirect"));
    let (program, probe) = http_program(vec![first, second]);
    let (observed, mut observations) = tokio::sync::mpsc::unbounded_channel();
    let runtime = LiveRuntime::builder(program)
        .bind_http()
        .bind_effect::<ObserveHttp, _>(HttpObserver {
            observations: observed,
        })
        .build()
        .expect("valid live HTTP bindings");
    let handle = runtime.handle(&probe).unwrap();
    let task = runtime.spawn();

    handle.send(HttpMessage::Start).await.unwrap();
    let seen = tokio::time::timeout(LIVE_TEST_TIMEOUT, observations.recv())
        .await
        .expect("HTTP sequence completes promptly")
        .expect("HTTP observation");
    assert_eq!(seen.len(), 2);
    let SeenHttp::Response(first) = &seen[0] else {
        panic!("first request did not return a response")
    };
    assert_eq!(first.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(first.headers()["x-response"], "status");
    assert_eq!(first.body(), &Bytes::from_static(b"unavailable"));
    let SeenHttp::Response(second) = &seen[1] else {
        panic!("second request did not return a response")
    };
    assert_eq!(second.status(), StatusCode::FOUND);
    assert_eq!(second.headers()["location"], "/must-not-follow");
    assert_eq!(second.body(), &Bytes::from_static(b"redirect"));

    assert!(task.shutdown(Shutdown::Drain).await.unwrap().is_clean());
    let report = tokio::time::timeout(LIVE_TEST_TIMEOUT, server)
        .await
        .expect("loopback server completes promptly")
        .unwrap()
        .unwrap();
    assert_eq!(report.accepted_connections, 1);
    assert_eq!(report.requests.len(), 2);
    assert!(report.requests[0].request_line.starts_with("POST /status "));
    assert!(
        report.requests[0]
            .headers
            .to_ascii_lowercase()
            .contains("x-request: present")
    );
    assert_eq!(
        report.requests[0]
            .headers
            .lines()
            .filter(|line| line.eq_ignore_ascii_case("x-order: first"))
            .count(),
        1
    );
    assert_eq!(
        report.requests[0]
            .headers
            .lines()
            .filter(|line| line.eq_ignore_ascii_case("x-order: second"))
            .count(),
        1
    );
    assert_eq!(report.requests[0].body, b"request body");
    assert!(
        report.requests[0]
            .headers
            .to_ascii_lowercase()
            .contains("accept: application/vnd.samara")
    );
    assert!(
        report.requests[1]
            .request_line
            .starts_with("GET /redirect ")
    );
    assert!(
        report.requests[1]
            .headers
            .to_ascii_lowercase()
            .contains("accept: */*")
    );
}

#[tokio::test]
async fn invalid_live_http_configuration_is_a_typed_effect_failure() {
    let (program, probe) = http_program(vec![HttpRequest::get("not a URL")]);
    let (observed, mut observations) = tokio::sync::mpsc::unbounded_channel();
    let runtime = LiveRuntime::builder(program)
        .bind_http()
        .bind_effect::<ObserveHttp, _>(HttpObserver {
            observations: observed,
        })
        .build()
        .unwrap();
    let handle = runtime.handle(&probe).unwrap();
    let task = runtime.spawn();

    handle.send(HttpMessage::Start).await.unwrap();
    let seen = tokio::time::timeout(LIVE_TEST_TIMEOUT, observations.recv())
        .await
        .expect("configuration outcome completes promptly")
        .unwrap();
    let [SeenHttp::Error(error)] = seen.as_slice() else {
        panic!("invalid URL did not produce one typed HTTP error")
    };
    assert_eq!(error.kind(), HttpErrorKind::Configuration);
    assert!(!error.message().is_empty());
    assert!(task.shutdown(Shutdown::Drain).await.unwrap().is_clean());
}

#[tokio::test]
async fn live_http_eof_is_a_typed_transport_failure() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        assert!(read_request(&mut stream).await.unwrap().is_some());
        drop(stream);
    });

    let request = HttpRequest::get(format!("http://{endpoint}/closed"));
    let (program, probe) = http_program(vec![request]);
    let (observed, mut observations) = tokio::sync::mpsc::unbounded_channel();
    let runtime = LiveRuntime::builder(program)
        .bind_http()
        .bind_effect::<ObserveHttp, _>(HttpObserver {
            observations: observed,
        })
        .build()
        .unwrap();
    let handle = runtime.handle(&probe).unwrap();
    let task = runtime.spawn();

    handle.send(HttpMessage::Start).await.unwrap();
    let seen = tokio::time::timeout(LIVE_TEST_TIMEOUT, observations.recv())
        .await
        .expect("transport outcome completes promptly")
        .unwrap();
    let [SeenHttp::Error(error)] = seen.as_slice() else {
        panic!("closed transport did not produce one typed HTTP error")
    };
    assert_eq!(error.kind(), HttpErrorKind::Transport);
    assert!(!error.message().is_empty());
    assert!(task.shutdown(Shutdown::Drain).await.unwrap().is_clean());
    tokio::time::timeout(LIVE_TEST_TIMEOUT, server)
        .await
        .expect("transport server completes promptly")
        .unwrap();
}

struct DuplicateHttpDriver;

impl EffectDriver<HttpRequest> for DuplicateHttpDriver {
    fn execute(&self, _descriptor: HttpRequest) -> BoxFuture<Result<HttpResponse, HttpError>> {
        Box::pin(async {
            Ok(HttpResponse::new(
                StatusCode::OK,
                Version::HTTP_11,
                HeaderMap::new(),
                Bytes::new(),
            ))
        })
    }
}

#[test]
fn bind_http_participates_in_normal_duplicate_binding_validation() {
    let (program, _) = http_program(vec![HttpRequest::get("https://example.test")]);
    assert!(
        LiveRuntime::builder(program)
            .bind_http()
            .bind_effect::<HttpRequest, _>(DuplicateHttpDriver)
            .build()
            .is_err()
    );
}

#[test]
fn http_errors_are_typed_and_fixture_constructible() {
    let configuration = HttpError::new(HttpErrorKind::Configuration, "bad URL");
    let transport = HttpError::new(HttpErrorKind::Transport, "connection closed");

    assert_eq!(configuration.kind(), HttpErrorKind::Configuration);
    assert_eq!(configuration.message(), "bad URL");
    assert_eq!(transport.kind(), HttpErrorKind::Transport);
    assert_eq!(transport.message(), "connection closed");
}
