//! Conformance evidence for Samara's standard `From` continuation convention.
//!
//! Each continuation-bearing boundary offers a short form using
//! `Message: From<BoundaryValue>` and a `_with` form for call-site-specific
//! mapping or captured domain context.

use std::convert::Infallible;

use bytes::Bytes;
use http::{HeaderMap, StatusCode, Version};
use samara::prelude::*;
use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProbeEffect;

impl EffectDescriptor for ProbeEffect {
    type Output = u64;
    type Error = Infallible;
}

#[derive(Debug, PartialEq, Eq)]
enum EffectMessage {
    Canonical(EffectOutcome<u64, Infallible>),
    Explicit {
        context: &'static str,
        outcome: EffectOutcome<u64, Infallible>,
    },
}

impl From<EffectOutcome<u64, Infallible>> for EffectMessage {
    fn from(outcome: EffectOutcome<u64, Infallible>) -> Self {
        Self::Canonical(outcome)
    }
}

#[test]
fn effect_default_uses_from_and_effect_with_preserves_context() {
    let mut program = Program::builder();
    let probe = program.effect::<ProbeEffect>();
    let default: Command<EffectMessage> = Command::effect(&probe, ProbeEffect);
    assert_eq!(
        default
            .map_effect_outcome::<ProbeEffect>(EffectOutcome::Succeeded(7))
            .ok()
            .expect("default effect command"),
        EffectMessage::Canonical(EffectOutcome::Succeeded(7))
    );

    let cancelled: Command<EffectMessage> = Command::effect(&probe, ProbeEffect);
    assert_eq!(
        cancelled
            .map_effect_outcome::<ProbeEffect>(EffectOutcome::Cancelled(CancelReason::Deadline))
            .ok()
            .expect("cancelled default effect command"),
        EffectMessage::Canonical(EffectOutcome::Cancelled(CancelReason::Deadline))
    );

    let context = "secondary read";
    let explicit: Command<EffectMessage> =
        Command::effect_with(&probe, ProbeEffect, move |outcome| {
            EffectMessage::Explicit { context, outcome }
        });
    assert_eq!(
        explicit
            .map_effect_outcome::<ProbeEffect>(EffectOutcome::Succeeded(8))
            .ok()
            .expect("explicit effect command"),
        EffectMessage::Explicit {
            context: "secondary read",
            outcome: EffectOutcome::Succeeded(8),
        }
    );

    let identity: Command<EffectOutcome<u64, Infallible>> = Command::effect(&probe, ProbeEffect);
    assert_eq!(
        identity
            .map_effect_outcome::<ProbeEffect>(EffectOutcome::Succeeded(9))
            .ok()
            .expect("identity effect command"),
        EffectOutcome::Succeeded(9)
    );
}

#[derive(Debug, PartialEq, Eq)]
enum SourceMessage {
    Canonical(SourceEvent<u64, Infallible>),
    Explicit {
        context: &'static str,
        event: SourceEvent<u64, Infallible>,
    },
}

impl From<SourceEvent<u64, Infallible>> for SourceMessage {
    fn from(event: SourceEvent<u64, Infallible>) -> Self {
        Self::Canonical(event)
    }
}

#[test]
fn source_default_is_reusable_and_source_with_preserves_context() {
    let mut program = Program::builder();
    let source = program.source::<StreamDescriptor<u64>>();
    let descriptor = StreamDescriptor::<u64>::named("from/default");
    let default: Subscription<SourceMessage> =
        Subscription::source(&source, SubscriptionId::new("default"), descriptor);
    assert_eq!(
        default
            .map_source_event::<StreamDescriptor<u64>>(SourceEvent::Item(1))
            .unwrap(),
        SourceMessage::Canonical(SourceEvent::Item(1))
    );
    assert_eq!(
        default
            .map_source_event::<StreamDescriptor<u64>>(SourceEvent::Ended)
            .unwrap(),
        SourceMessage::Canonical(SourceEvent::Ended)
    );

    let context = "telemetry";
    let explicit = Subscription::source_with(
        &source,
        SubscriptionId::new("explicit"),
        StreamDescriptor::<u64>::named("from/explicit"),
        move |event| SourceMessage::Explicit { context, event },
    );
    assert_eq!(
        explicit
            .map_source_event::<StreamDescriptor<u64>>(SourceEvent::Item(3))
            .unwrap(),
        SourceMessage::Explicit {
            context: "telemetry",
            event: SourceEvent::Item(3),
        }
    );
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct Payload {
    value: u64,
}

enum HttpMessage {
    Canonical(EffectOutcome<Payload, HttpResponseError>),
    Explicit(&'static str),
}

impl From<EffectOutcome<Payload, HttpResponseError>> for HttpMessage {
    fn from(outcome: EffectOutcome<Payload, HttpResponseError>) -> Self {
        Self::Canonical(outcome)
    }
}

fn http_response(body: &'static [u8]) -> HttpResponse {
    HttpResponse::new(
        StatusCode::OK,
        Version::HTTP_11,
        HeaderMap::new(),
        Bytes::from_static(body),
    )
}

#[test]
fn http_pipeline_default_uses_from_and_into_command_with_is_explicit() {
    let mut program = Program::builder();
    let http = program.effect::<HttpRequest>();
    let default: Command<HttpMessage> = HttpRequest::get("https://example.test/default")
        .on_response()
        .json::<Payload>()
        .into_command(&http);
    let invocation = default.into_effect::<HttpRequest>().ok().unwrap();
    let message =
        invocation.map_outcome(EffectOutcome::Succeeded(http_response(br#"{"value": 41}"#)));
    let HttpMessage::Canonical(EffectOutcome::Succeeded(payload)) = message else {
        panic!("the default HTTP continuation did not use Message::from")
    };
    assert_eq!(payload, Payload { value: 41 });

    let context = "secondary endpoint";
    let explicit: Command<HttpMessage> = HttpRequest::get("https://example.test/explicit")
        .on_response()
        .json::<Payload>()
        .into_command_with(&http, move |_| HttpMessage::Explicit(context));
    let invocation = explicit.into_effect::<HttpRequest>().ok().unwrap();
    assert!(matches!(
        invocation.map_outcome(EffectOutcome::Succeeded(http_response(br#"{"value": 42}"#))),
        HttpMessage::Explicit("secondary endpoint")
    ));
}

struct EchoProtocol;

enum EchoProtocolMessage {
    Echo(RequestInvocation<EchoProtocol, Echo>),
}

impl Protocol for EchoProtocol {
    type Message = EchoProtocolMessage;
}

struct Echo(u64);

impl Request<EchoProtocol> for Echo {
    type Reply = u64;

    fn into_message(self, reply_to: ReplyTo<Self::Reply>) -> EchoProtocolMessage {
        EchoProtocolMessage::Echo(RequestInvocation::new(self, reply_to))
    }
}

enum ProviderMessage {
    Protocol(EchoProtocolMessage),
}

impl From<EchoProtocolMessage> for ProviderMessage {
    fn from(message: EchoProtocolMessage) -> Self {
        Self::Protocol(message)
    }
}

struct EchoProvider;

impl Component for EchoProvider {
    type Model = ();
    type Message = ProviderMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(())
    }

    fn update(&self, _model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        let ProviderMessage::Protocol(EchoProtocolMessage::Echo(invocation)) = message;
        Command::reply(invocation.reply_to, invocation.request.0)
    }
}

#[derive(Default)]
struct RequesterModel {
    canonical: Option<RequestOutcome<u64>>,
    explicit: Option<(&'static str, RequestOutcome<u64>)>,
}

enum RequesterMessage {
    StartDefault,
    StartExplicit,
    Canonical(RequestOutcome<u64>),
    Explicit {
        context: &'static str,
        outcome: RequestOutcome<u64>,
    },
}

impl From<RequestOutcome<u64>> for RequesterMessage {
    fn from(outcome: RequestOutcome<u64>) -> Self {
        Self::Canonical(outcome)
    }
}

struct Requester {
    echo: Port<EchoProtocol>,
}

impl Component for Requester {
    type Model = RequesterModel;
    type Message = RequesterMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(RequesterModel::default())
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            RequesterMessage::StartDefault => Command::request(self.echo.clone(), Echo(11)),
            RequesterMessage::StartExplicit => {
                let context = "second request";
                Command::request_with(self.echo.clone(), Echo(12), move |outcome| {
                    RequesterMessage::Explicit { context, outcome }
                })
            }
            RequesterMessage::Canonical(outcome) => {
                model.canonical = Some(outcome);
                Command::none()
            }
            RequesterMessage::Explicit { context, outcome } => {
                model.explicit = Some((context, outcome));
                Command::none()
            }
        }
    }
}

#[test]
fn request_default_uses_from_and_request_with_preserves_context() {
    let mut builder = Program::builder();
    let echo = builder.port::<EchoProtocol>(PortId::new("echo"));
    let provider = builder.component(ComponentId::new("provider"), EchoProvider);
    builder.bind_port(&echo, &provider);
    let requester = builder.component(
        ComponentId::new("requester"),
        Requester { echo: echo.clone() },
    );
    let mut runtime = ControlledRuntime::builder(builder.build().unwrap())
        .build()
        .unwrap();

    runtime
        .send(&requester, RequesterMessage::StartDefault)
        .unwrap();
    runtime
        .send(&requester, RequesterMessage::StartExplicit)
        .unwrap();
    runtime.run_until_idle().unwrap();

    let model = runtime.state(&requester).unwrap();
    assert_eq!(model.canonical, Some(RequestOutcome::Replied(11)));
    assert_eq!(
        model.explicit,
        Some(("second request", RequestOutcome::Replied(12)))
    );
    assert!(runtime.cancel().unwrap().is_clean());
}
