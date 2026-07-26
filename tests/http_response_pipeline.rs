use std::{
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, StatusCode, Version};
use samara::prelude::*;
use serde::{Deserialize, Deserializer, de::IgnoredAny};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

const LIVE_TEST_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct Payload {
    value: u64,
}

fn response(status: StatusCode, body: &'static [u8]) -> HttpResponse {
    let mut headers = HeaderMap::new();
    headers.insert("x-response", HeaderValue::from_static("preserved"));
    HttpResponse::new(status, Version::HTTP_11, headers, Bytes::from_static(body))
}

fn json_command<T>() -> Command<EffectOutcome<T, HttpResponseError>>
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    let mut program = Program::builder();
    let http = program.effect::<HttpRequest>();
    HttpRequest::get("https://example.test/value")
        .on_response()
        .json::<T>()
        .into_command_with(&http, |outcome| outcome)
}

fn successful_json_command<T>() -> Command<EffectOutcome<T, HttpResponseError>>
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    let mut program = Program::builder();
    let http = program.effect::<HttpRequest>();
    HttpRequest::get("https://example.test/value")
        .on_response()
        .require_success()
        .json::<T>()
        .into_command_with(&http, |outcome| outcome)
}

#[test]
fn http_response_pipeline_preserves_the_owned_raw_request_boundary() {
    let mut program = Program::builder();
    let http = program.effect::<HttpRequest>();
    let command: Command<()> = HttpRequest::get("https://example.test/owned")
        .with_header(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        )
        .with_body(Bytes::from_static(b"owned request"))
        .on_response()
        .into_command_with(&http, |_| ());

    let invocation = command
        .into_effect::<HttpRequest>()
        .ok()
        .expect("the pipeline lowers to its raw HTTP request");
    assert_eq!(invocation.descriptor().url(), "https://example.test/owned");
    assert_eq!(invocation.descriptor().body(), b"owned request".as_slice());
    assert_eq!(
        invocation.descriptor().headers()[http::header::CONTENT_TYPE],
        "application/octet-stream"
    );
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PipelineResult {
    Succeeded(u64),
    Http(HttpErrorKind),
    Status(StatusCode),
    Json,
    OtherResponseFailure,
    Cancelled(CancelReason),
}

#[derive(Default)]
struct PipelineModel {
    result: Option<PipelineResult>,
}

enum PipelineMessage {
    Start,
    Finished(EffectOutcome<Payload, HttpResponseError>),
    Observed,
}

struct PipelineComponent {
    http: EffectCapability<HttpRequest>,
    observe: EffectCapability<ObservePipelineResult>,
    url: String,
}

impl Component for PipelineComponent {
    type Model = PipelineModel;
    type Message = PipelineMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(PipelineModel::default())
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            PipelineMessage::Start => HttpRequest::get(self.url.clone())
                .on_response()
                .require_success()
                .json::<Payload>()
                .into_command_with(&self.http, PipelineMessage::Finished),
            PipelineMessage::Finished(outcome) => {
                let result = match outcome {
                    EffectOutcome::Succeeded(payload) => PipelineResult::Succeeded(payload.value),
                    EffectOutcome::Failed(HttpResponseError::Http(error)) => {
                        PipelineResult::Http(error.kind())
                    }
                    EffectOutcome::Failed(HttpResponseError::Status(error)) => {
                        PipelineResult::Status(error.response().status())
                    }
                    EffectOutcome::Failed(HttpResponseError::Json(_)) => PipelineResult::Json,
                    EffectOutcome::Failed(_) => PipelineResult::OtherResponseFailure,
                    EffectOutcome::Cancelled(reason) => PipelineResult::Cancelled(reason),
                };
                model.result = Some(result.clone());
                Command::effect_with(&self.observe, ObservePipelineResult(result), |_| {
                    PipelineMessage::Observed
                })
            }
            PipelineMessage::Observed => Command::none(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ObservePipelineResult(PipelineResult);

impl EffectDescriptor for ObservePipelineResult {
    type Output = ();
    type Error = Infallible;
}

fn pipeline_program(url: impl Into<String>) -> (Program, ComponentRef<PipelineComponent>) {
    let mut builder = Program::builder();
    let http = builder.effect::<HttpRequest>();
    let observe = builder.effect::<ObservePipelineResult>();
    let component = builder.component(
        ComponentId::new("http-pipeline"),
        PipelineComponent {
            http,
            observe,
            url: url.into(),
        },
    );
    (builder.build().expect("valid pipeline program"), component)
}

#[test]
fn controlled_http_pipeline_intercepts_raw_request_and_decodes_2xx_json() {
    let (program, component) = pipeline_program("not a live URL");
    let mut runtime = ControlledRuntime::builder(program)
        .control_effect::<HttpRequest>()
        .control_effect::<ObservePipelineResult>()
        .build()
        .expect("controlled pipeline bindings");

    runtime.send(&component, PipelineMessage::Start).unwrap();
    runtime.run_until_idle().unwrap();
    let pending = runtime.next_effect::<HttpRequest>().unwrap();
    assert_eq!(pending.intent.url(), "not a live URL");
    runtime
        .complete(
            pending,
            EffectOutcome::Succeeded(response(StatusCode::OK, br#"{"value": 41}"#)),
        )
        .unwrap();
    runtime.run_until_idle().unwrap();

    assert_eq!(
        runtime.state(&component).unwrap().result,
        Some(PipelineResult::Succeeded(41))
    );
    let observation = runtime.next_effect::<ObservePipelineResult>().unwrap();
    assert_eq!(observation.intent.0, PipelineResult::Succeeded(41));
    runtime
        .complete(observation, EffectOutcome::Succeeded(()))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.cancel().unwrap().is_clean());
}

#[test]
fn json_without_status_policy_decodes_a_non_success_response() {
    let invocation = json_command::<Payload>()
        .into_effect::<HttpRequest>()
        .ok()
        .unwrap();
    let outcome = invocation.map_outcome(EffectOutcome::Succeeded(response(
        StatusCode::UNPROCESSABLE_ENTITY,
        br#"{"value": 17}"#,
    )));

    let EffectOutcome::Succeeded(payload) = outcome else {
        panic!("valid non-2xx JSON did not decode successfully")
    };
    assert_eq!(payload, Payload { value: 17 });
}

static REJECTED_DECODE_CALLS: AtomicUsize = AtomicUsize::new(0);

struct RejectedDecode;

impl<'de> Deserialize<'de> for RejectedDecode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        REJECTED_DECODE_CALLS.fetch_add(1, Ordering::SeqCst);
        IgnoredAny::deserialize(deserializer)?;
        Ok(Self)
    }
}

#[test]
fn require_success_rejects_before_json_decoding() {
    REJECTED_DECODE_CALLS.store(0, Ordering::SeqCst);
    let expected = response(StatusCode::BAD_GATEWAY, br#"{"value": 17}"#);
    let invocation = successful_json_command::<RejectedDecode>()
        .into_effect::<HttpRequest>()
        .ok()
        .unwrap();
    let outcome = invocation.map_outcome(EffectOutcome::Succeeded(expected.clone()));

    let EffectOutcome::Failed(HttpResponseError::Status(error)) = outcome else {
        panic!("non-2xx response did not fail status policy")
    };
    assert_eq!(REJECTED_DECODE_CALLS.load(Ordering::SeqCst), 0);
    assert_eq!(error.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(error.response(), &expected);
    assert_eq!(error.into_response(), expected);
}

#[test]
fn json_failure_retains_source_and_complete_response() {
    let expected = response(StatusCode::OK, br#"{"value": }"#);
    let invocation = successful_json_command::<Payload>()
        .into_effect::<HttpRequest>()
        .ok()
        .unwrap();
    let outcome = invocation.map_outcome(EffectOutcome::Succeeded(expected.clone()));

    let EffectOutcome::Failed(HttpResponseError::Json(error)) = outcome else {
        panic!("invalid JSON did not produce a JSON response error")
    };
    assert!(error.source().is_syntax());
    assert!(std::error::Error::source(&error).is_some());
    assert_eq!(error.response().status(), StatusCode::OK);
    assert_eq!(error.response().headers()["x-response"], "preserved");
    assert_eq!(error.response().body(), br#"{"value": }"#.as_slice());
    let (source, retained) = error.into_parts();
    assert!(source.is_syntax());
    assert_eq!(retained, expected);
}

static BYPASSED_DECODE_CALLS: AtomicUsize = AtomicUsize::new(0);

struct BypassedDecode;

impl<'de> Deserialize<'de> for BypassedDecode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        BYPASSED_DECODE_CALLS.fetch_add(1, Ordering::SeqCst);
        IgnoredAny::deserialize(deserializer)?;
        Ok(Self)
    }
}

#[test]
fn http_failure_and_cancellation_preserve_distinct_outcome_channels() {
    BYPASSED_DECODE_CALLS.store(0, Ordering::SeqCst);
    let expected = HttpError::new(HttpErrorKind::Transport, "connection reset");
    let invocation = successful_json_command::<BypassedDecode>()
        .into_effect::<HttpRequest>()
        .ok()
        .unwrap();
    let outcome = invocation.map_outcome(EffectOutcome::Failed(expected.clone()));
    let EffectOutcome::Failed(HttpResponseError::Http(actual)) = outcome else {
        panic!("raw HTTP failure was not retained")
    };
    assert_eq!(actual, expected);

    let invocation = successful_json_command::<BypassedDecode>()
        .into_effect::<HttpRequest>()
        .ok()
        .unwrap();
    let outcome = invocation.map_outcome(EffectOutcome::Cancelled(CancelReason::Deadline));
    assert!(matches!(
        outcome,
        EffectOutcome::Cancelled(CancelReason::Deadline)
    ));
    assert_eq!(BYPASSED_DECODE_CALLS.load(Ordering::SeqCst), 0);
}

static ONCE_DECODE_CALLS: AtomicUsize = AtomicUsize::new(0);

struct OnceDecoded;

impl<'de> Deserialize<'de> for OnceDecoded {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        ONCE_DECODE_CALLS.fetch_add(1, Ordering::SeqCst);
        IgnoredAny::deserialize(deserializer)?;
        Ok(Self)
    }
}

#[test]
fn http_pipeline_transforms_and_maps_at_most_once() {
    let mut program = Program::builder();
    let http = program.effect::<HttpRequest>();
    ONCE_DECODE_CALLS.store(0, Ordering::SeqCst);
    let mapper_calls = Arc::new(AtomicUsize::new(0));
    let counted_mapper_calls = mapper_calls.clone();
    let command = HttpRequest::get("https://example.test/once")
        .on_response()
        .require_success()
        .json::<OnceDecoded>()
        .into_command_with(&http, move |outcome| {
            counted_mapper_calls.fetch_add(1, Ordering::SeqCst);
            outcome
        });
    let invocation = command
        .into_effect::<HttpRequest>()
        .ok()
        .expect("raw effect invocation");
    let outcome = invocation.map_outcome(EffectOutcome::Succeeded(response(
        StatusCode::OK,
        br#"{"ignored": true}"#,
    )));

    assert!(matches!(outcome, EffectOutcome::Succeeded(OnceDecoded)));
    assert_eq!(ONCE_DECODE_CALLS.load(Ordering::SeqCst), 1);
    assert_eq!(mapper_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn controlled_trace_records_raw_success_when_json_mapper_fails() {
    let (program, component) = pipeline_program("https://example.test/invalid");
    let mut runtime = ControlledRuntime::builder(program)
        .control_effect::<HttpRequest>()
        .control_effect::<ObservePipelineResult>()
        .build()
        .unwrap();
    runtime.send(&component, PipelineMessage::Start).unwrap();
    runtime.run_until_idle().unwrap();
    let pending = runtime.next_effect::<HttpRequest>().unwrap();
    runtime
        .complete(
            pending,
            EffectOutcome::Succeeded(response(StatusCode::OK, b"not JSON")),
        )
        .unwrap();
    runtime.run_until_idle().unwrap();

    assert_eq!(
        runtime.state(&component).unwrap().result,
        Some(PipelineResult::Json)
    );
    assert!(runtime.trace().iter().any(|record| {
        matches!(
            record.event,
            TraceEvent::EffectOutcome {
                effect_type,
                outcome: EffectOutcomeKind::Succeeded,
                ..
            } if effect_type == std::any::type_name::<HttpRequest>()
        )
    }));
    assert_eq!(
        runtime
            .trace()
            .iter()
            .filter(|record| matches!(record.event, TraceEvent::EffectOutcome { .. }))
            .count(),
        1
    );
}

struct PipelineObserver {
    observed: tokio::sync::mpsc::UnboundedSender<PipelineResult>,
}

impl EffectDriver<ObservePipelineResult> for PipelineObserver {
    fn execute(&self, descriptor: ObservePipelineResult) -> BoxFuture<Result<(), Infallible>> {
        let observed = self.observed.clone();
        Box::pin(async move {
            let _ = observed.send(descriptor.0);
            Ok(())
        })
    }
}

async fn serve_json_once(listener: TcpListener) -> std::io::Result<()> {
    let (mut stream, _) = listener.accept().await?;
    let mut request = Vec::new();
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let mut chunk = [0_u8; 1024];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "request ended before headers",
            ));
        }
        request.extend_from_slice(&chunk[..read]);
    }
    stream
        .write_all(
            concat!(
                "HTTP/1.1 200 OK\r\n",
                "Content-Type: application/json\r\n",
                "Content-Length: 13\r\n",
                "Connection: close\r\n",
                "\r\n",
                "{\"value\": 29}"
            )
            .as_bytes(),
        )
        .await?;
    stream.flush().await
}

#[tokio::test]
async fn live_and_controlled_http_pipelines_produce_equivalent_component_results() {
    let controlled_result = {
        let (program, component) = pipeline_program("https://controlled.test/value");
        let mut runtime = ControlledRuntime::builder(program)
            .control_effect::<HttpRequest>()
            .control_effect::<ObservePipelineResult>()
            .build()
            .unwrap();
        runtime.send(&component, PipelineMessage::Start).unwrap();
        runtime.run_until_idle().unwrap();
        let pending = runtime.next_effect::<HttpRequest>().unwrap();
        runtime
            .complete(
                pending,
                EffectOutcome::Succeeded(response(StatusCode::OK, br#"{"value": 29}"#)),
            )
            .unwrap();
        runtime.run_until_idle().unwrap();
        runtime
            .next_effect::<ObservePipelineResult>()
            .unwrap()
            .intent
            .0
            .clone()
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    let server = tokio::spawn(serve_json_once(listener));
    let (program, component) = pipeline_program(format!("http://{endpoint}/value"));
    let (observed, mut observations) = tokio::sync::mpsc::unbounded_channel();
    let runtime = LiveRuntime::builder(program)
        .bind_http()
        .bind_effect::<ObservePipelineResult, _>(PipelineObserver { observed })
        .build()
        .unwrap();
    let handle = runtime.handle(&component).unwrap();
    let task = runtime.spawn();
    handle.send(PipelineMessage::Start).await.unwrap();
    let live_result = tokio::time::timeout(LIVE_TEST_TIMEOUT, observations.recv())
        .await
        .expect("live response pipeline completes promptly")
        .expect("live observation");

    assert_eq!(live_result, controlled_result);
    assert_eq!(live_result, PipelineResult::Succeeded(29));
    assert!(task.shutdown(Shutdown::Drain).await.unwrap().is_clean());
    tokio::time::timeout(LIVE_TEST_TIMEOUT, server)
        .await
        .expect("loopback server completes promptly")
        .unwrap()
        .unwrap();
}
