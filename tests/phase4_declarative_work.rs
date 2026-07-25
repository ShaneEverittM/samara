//! Executable acceptance evidence for the Phase 4 declarative-work kernel.
//!
//! These tests stop before either execution profile. They inspect and map
//! inert effect Commands, reuse Subscription message mappers, and exercise the
//! pure framed Source Layer without selecting a Driver or controlled binding.

use std::convert::Infallible;

use samara::prelude::*;

#[derive(Debug, PartialEq, Eq)]
struct PersistValue {
    value: u64,
}

impl EffectDescriptor for PersistValue {
    type Output = u64;
    type Error = PersistError;
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PersistError;

#[derive(Debug, PartialEq, Eq)]
enum EffectMessage {
    Persisted {
        request: u64,
        outcome: EffectOutcome<u64, PersistError>,
    },
}

#[test]
fn v3_effect_command_exposes_descriptor_and_maps_one_outcome() {
    let command = Command::effect_with(PersistValue { value: 41 }, |outcome| {
        EffectMessage::Persisted {
            request: 41,
            outcome,
        }
    });

    let invocation = match command.into_effect::<PersistValue>() {
        Ok(invocation) => invocation,
        Err(_) => panic!("the concrete descriptor type should match"),
    };
    assert_eq!(invocation.descriptor(), &PersistValue { value: 41 });
    let mapped = invocation.map_outcome(EffectOutcome::Succeeded(42));

    assert_eq!(
        mapped,
        EffectMessage::Persisted {
            request: 41,
            outcome: EffectOutcome::Succeeded(42),
        }
    );
}

#[test]
fn v3_all_effect_outcomes_remain_typed_message_input() {
    let failed = Command::effect_with(PersistValue { value: 5 }, |outcome| {
        EffectMessage::Persisted {
            request: 5,
            outcome,
        }
    })
    .map_effect_outcome::<PersistValue>(EffectOutcome::Failed(PersistError))
    .unwrap_or_else(|_| panic!("failure descriptor should match"));
    let cancelled = Command::effect_with(PersistValue { value: 6 }, |outcome| {
        EffectMessage::Persisted {
            request: 6,
            outcome,
        }
    })
    .map_effect_outcome::<PersistValue>(EffectOutcome::Cancelled(CancelReason::Shutdown))
    .unwrap_or_else(|_| panic!("cancelled descriptor should match"));

    assert_eq!(
        failed,
        EffectMessage::Persisted {
            request: 5,
            outcome: EffectOutcome::Failed(PersistError),
        }
    );
    assert_eq!(
        cancelled,
        EffectMessage::Persisted {
            request: 6,
            outcome: EffectOutcome::Cancelled(CancelReason::Shutdown),
        }
    );
}

#[test]
fn v3_equal_effect_descriptors_remain_distinct_command_occurrences() {
    let command: Command<EffectMessage> = Command::batch([
        Command::effect_with(PersistValue { value: 7 }, |outcome| {
            EffectMessage::Persisted {
                request: 1,
                outcome,
            }
        }),
        Command::effect_with(PersistValue { value: 7 }, |outcome| {
            EffectMessage::Persisted {
                request: 2,
                outcome,
            }
        }),
    ]);

    assert_eq!(
        command.effect_intents::<PersistValue>(),
        vec![&PersistValue { value: 7 }, &PersistValue { value: 7 }]
    );

    let mut declarations = command.into_declarations().into_iter();
    let first = declarations.next().expect("first effect declaration");
    let second = declarations.next().expect("second effect declaration");
    assert!(declarations.next().is_none());

    let first = first
        .map_effect_outcome::<PersistValue>(EffectOutcome::Succeeded(70))
        .unwrap_or_else(|_| panic!("first descriptor should match"));
    let second = second
        .map_effect_outcome::<PersistValue>(EffectOutcome::Succeeded(71))
        .unwrap_or_else(|_| panic!("second descriptor should match"));
    assert_eq!(
        first,
        EffectMessage::Persisted {
            request: 1,
            outcome: EffectOutcome::Succeeded(70),
        }
    );
    assert_eq!(
        second,
        EffectMessage::Persisted {
            request: 2,
            outcome: EffectOutcome::Succeeded(71),
        }
    );
}

#[derive(Debug, PartialEq, Eq)]
enum StreamMessage {
    Input(SourceEvent<u64, Infallible>),
}

#[test]
fn v4_source_event_mapper_is_reusable_before_delivery() {
    let subscription = Subscription::source_with(
        SubscriptionId::new("input"),
        StreamDescriptor::<u64>::named("phase4/input"),
        StreamMessage::Input,
    );

    assert_eq!(
        subscription
            .map_source_event::<StreamDescriptor<u64>>(SourceEvent::Item(3))
            .expect("the concrete descriptor type should match"),
        StreamMessage::Input(SourceEvent::Item(3))
    );
    assert_eq!(
        subscription
            .map_source_event::<StreamDescriptor<u64>>(SourceEvent::Item(5))
            .expect("the same mapper remains reusable"),
        StreamMessage::Input(SourceEvent::Item(5))
    );
    assert_eq!(
        subscription
            .map_source_event::<StreamDescriptor<u64>>(SourceEvent::Ended)
            .expect("terminal events use the same mapper"),
        StreamMessage::Input(SourceEvent::Ended)
    );
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RawChunks {
    endpoint: &'static str,
}

impl SourceDescriptor for RawChunks {
    type Item = Vec<u8>;
    type Error = TransportError;
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TransportError(&'static str);

#[derive(Clone, Debug, PartialEq, Eq)]
struct LengthDelimited {
    maximum: usize,
}

#[derive(Default)]
struct DecoderState {
    buffered: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DecodeError;

impl Decoder for LengthDelimited {
    type Chunk = Vec<u8>;
    type Frame = Vec<u8>;
    type Error = DecodeError;
    type State = DecoderState;

    fn start(&self) -> Self::State {
        DecoderState::default()
    }

    fn push(
        &self,
        state: &mut Self::State,
        chunk: Self::Chunk,
    ) -> Result<Vec<Self::Frame>, Self::Error> {
        state.buffered.extend(chunk);
        let mut frames = Vec::new();

        while let Some((&length, payload)) = state.buffered.split_first() {
            let length = usize::from(length);
            if length > self.maximum {
                return Err(DecodeError);
            }
            if payload.len() < length {
                break;
            }

            frames.push(payload[..length].to_vec());
            state.buffered.drain(..=length);
        }

        Ok(frames)
    }

    fn finish(&self, state: &mut Self::State) -> Result<Vec<Self::Frame>, Self::Error> {
        if state.buffered.is_empty() {
            Ok(Vec::new())
        } else {
            Err(DecodeError)
        }
    }
}

type FramedChunks = Framed<RawChunks, LengthDelimited>;
type FramedChunkEvent = SourceEvent<Vec<u8>, FramedError<TransportError, DecodeError>>;

#[derive(Debug, PartialEq, Eq)]
enum FrameMessage {
    Socket(FramedChunkEvent),
}

#[test]
fn framed_source_layer_is_profile_independent_and_stateful() {
    let descriptor = Framed::new(
        RawChunks {
            endpoint: "telemetry:7000",
        },
        LengthDelimited { maximum: 8 },
    );
    let mut layer = descriptor.into_layer();

    assert_eq!(
        layer.inner_descriptor(),
        &RawChunks {
            endpoint: "telemetry:7000"
        }
    );
    assert_eq!(
        layer.map_event(SourceEvent::Item(vec![3, b'a'])),
        Vec::<FramedChunkEvent>::new()
    );
    assert_eq!(
        layer.map_event(SourceEvent::Item(vec![b'b', b'c', 1, b'x'])),
        vec![
            SourceEvent::Item(b"abc".to_vec()),
            SourceEvent::Item(b"x".to_vec()),
        ]
    );
    assert_eq!(
        layer.map_event(SourceEvent::Failed(TransportError("reset"))),
        vec![SourceEvent::Failed(FramedError::Source(TransportError(
            "reset"
        )))]
    );
    assert_eq!(
        layer.map_event(SourceEvent::Ended),
        vec![SourceEvent::Ended]
    );
}

#[test]
fn framed_source_layer_preserves_decode_failure_as_typed_data() {
    let mut layer = FramedChunks::new(
        RawChunks {
            endpoint: "telemetry:7000",
        },
        LengthDelimited { maximum: 2 },
    )
    .into_layer();

    assert_eq!(
        layer.map_event(SourceEvent::Item(vec![3, b'a', b'b', b'c'])),
        vec![SourceEvent::Failed(FramedError::Decode(DecodeError))]
    );
}

#[test]
fn framed_layers_compose_without_profile_binding() {
    let inner = FramedChunks::new(
        RawChunks {
            endpoint: "telemetry:7000",
        },
        LengthDelimited { maximum: 8 },
    );
    let outer = Framed::new(inner.clone(), LengthDelimited { maximum: 8 });
    let mut inner_layer = inner.into_layer();
    let mut outer_layer = outer.into_layer();

    let outer_events = inner_layer
        .map_event(SourceEvent::Item(vec![3, 2, b'a', b'b']))
        .into_iter()
        .flat_map(|event| outer_layer.map_event(event))
        .collect::<Vec<_>>();

    assert_eq!(outer_events, vec![SourceEvent::Item(b"ab".to_vec())]);
}

#[test]
fn v4_framed_source_events_map_into_component_messages() {
    let descriptor = FramedChunks::new(
        RawChunks {
            endpoint: "telemetry:7000",
        },
        LengthDelimited { maximum: 8 },
    );
    let mut layer = descriptor.clone().into_layer();
    let subscription = Subscription::source_with(
        SubscriptionId::new("socket"),
        descriptor,
        FrameMessage::Socket,
    );

    let messages = layer
        .map_event(SourceEvent::Item(vec![1, b'x', 1, b'y']))
        .into_iter()
        .map(|event| {
            subscription
                .map_source_event::<FramedChunks>(event)
                .expect("the composed descriptor should match")
        })
        .collect::<Vec<_>>();

    assert_eq!(
        messages,
        vec![
            FrameMessage::Socket(SourceEvent::Item(b"x".to_vec())),
            FrameMessage::Socket(SourceEvent::Item(b"y".to_vec())),
        ]
    );

    let failure_events = layer.map_event(SourceEvent::Failed(TransportError("reset")));
    let [failure] = failure_events.as_slice() else {
        panic!("an inner failure should produce one outer terminal event");
    };
    let failure = subscription
        .map_source_event::<FramedChunks>(failure.clone())
        .expect("the composed descriptor should map its failure");
    assert_eq!(
        failure,
        FrameMessage::Socket(SourceEvent::Failed(FramedError::Source(TransportError(
            "reset"
        ))))
    );
}

#[derive(Clone)]
struct EofDecoder {
    finish_calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    fail_push: bool,
    fail_finish: bool,
}

impl std::fmt::Debug for EofDecoder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EofDecoder")
            .field("fail_push", &self.fail_push)
            .field("fail_finish", &self.fail_finish)
            .finish()
    }
}

impl PartialEq for EofDecoder {
    fn eq(&self, other: &Self) -> bool {
        self.fail_push == other.fail_push && self.fail_finish == other.fail_finish
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EofError;

impl Decoder for EofDecoder {
    type Chunk = Vec<u8>;
    type Frame = u8;
    type Error = EofError;
    type State = Vec<u8>;

    fn start(&self) -> Self::State {
        Vec::new()
    }

    fn push(
        &self,
        state: &mut Self::State,
        chunk: Self::Chunk,
    ) -> Result<Vec<Self::Frame>, Self::Error> {
        if self.fail_push {
            return Err(EofError);
        }
        state.extend(chunk);
        Ok(Vec::new())
    }

    fn finish(&self, state: &mut Self::State) -> Result<Vec<Self::Frame>, Self::Error> {
        self.finish_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.fail_finish {
            Err(EofError)
        } else {
            Ok(std::mem::take(state))
        }
    }
}

fn eof_decoder(
    fail_push: bool,
    fail_finish: bool,
) -> (EofDecoder, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    let finish_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    (
        EofDecoder {
            finish_calls: finish_calls.clone(),
            fail_push,
            fail_finish,
        },
        finish_calls,
    )
}

#[test]
fn phase6_framed_eof_emits_final_frames_before_ended() {
    let (decoder, finish_calls) = eof_decoder(false, false);
    let mut layer = Framed::new(
        RawChunks {
            endpoint: "telemetry:7000",
        },
        decoder,
    )
    .into_layer();
    assert!(layer.map_event(SourceEvent::Item(vec![1, 2])).is_empty());
    assert_eq!(
        layer.map_event(SourceEvent::Ended),
        vec![
            SourceEvent::Item(1),
            SourceEvent::Item(2),
            SourceEvent::Ended,
        ]
    );
    assert_eq!(finish_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn phase6_framed_finish_failure_emits_failed_without_ended() {
    let (decoder, finish_calls) = eof_decoder(false, true);
    let mut layer = Framed::new(
        RawChunks {
            endpoint: "telemetry:7000",
        },
        decoder,
    )
    .into_layer();
    assert!(layer.map_event(SourceEvent::Item(vec![1])).is_empty());
    assert_eq!(
        layer.map_event(SourceEvent::Ended),
        vec![SourceEvent::Failed(FramedError::Decode(EofError))]
    );
    assert_eq!(finish_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn phase6_framed_source_failure_does_not_run_finish() {
    let (decoder, finish_calls) = eof_decoder(false, false);
    let mut layer = Framed::new(
        RawChunks {
            endpoint: "telemetry:7000",
        },
        decoder,
    )
    .into_layer();
    assert_eq!(
        layer.map_event(SourceEvent::Failed(TransportError("reset"))),
        vec![SourceEvent::Failed(FramedError::Source(TransportError(
            "reset"
        )))]
    );
    assert_eq!(finish_calls.load(std::sync::atomic::Ordering::SeqCst), 0);

    let (decoder, finish_calls) = eof_decoder(true, false);
    let mut layer = Framed::new(
        RawChunks {
            endpoint: "telemetry:7000",
        },
        decoder,
    )
    .into_layer();
    assert_eq!(
        layer.map_event(SourceEvent::Item(vec![1])),
        vec![SourceEvent::Failed(FramedError::Decode(EofError))]
    );
    assert_eq!(finish_calls.load(std::sync::atomic::Ordering::SeqCst), 0);
}
