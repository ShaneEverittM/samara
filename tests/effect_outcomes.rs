//! ADR-0020 acceptance: success-specific Messages and one failure-only Message
//! keep routing separate from domain mapping, without losing trace kinds.
//! ADR-0025 acceptance: an effect that cannot fail has an irrefutable outcome.

use std::time::{Duration, SystemTime};

use samara::prelude::*;

struct ReadText;

impl EffectDescriptor for ReadText {
    type Output = String;
    type Error = &'static str;
}

struct CountBytes;

impl EffectDescriptor for CountBytes {
    type Output = usize;
    type Error = &'static str;
}

#[derive(Debug, PartialEq, Eq)]
enum Observation {
    Text(String),
    Count(usize),
    BackendError(&'static str),
}

enum Message {
    Read,
    Count,
    Text(String),
    Counted(usize),
    Failed(&'static str),
}

struct Reader;

impl Component for Reader {
    type Requires = (Effect<ReadText>, Effect<CountBytes>);
    type Config = ();
    type Model = Vec<Observation>;
    type Message = Message;

    fn init(_: &(), _: &Capabilities<Self::Requires>) -> Init<Self::Model, Message> {
        Init::default()
    }

    fn update(
        _: &(),
        (read, count): &Capabilities<Self::Requires>,
        model: &mut Self::Model,
        message: Message,
    ) -> Command<Message> {
        let observed = match message {
            Message::Read => {
                return Effect::perform(read, ReadText).producing(|outcome| match outcome {
                    EffectOutcome::Succeeded(text) => Message::Text(text),
                    EffectOutcome::Failed(reason) => Message::Failed(reason),
                });
            }
            Message::Count => {
                return Effect::perform(count, CountBytes).producing(|outcome| match outcome {
                    EffectOutcome::Succeeded(count) => Message::Counted(count),
                    EffectOutcome::Failed(reason) => Message::Failed(reason),
                });
            }
            Message::Text(text) => Observation::Text(text),
            Message::Counted(count) => Observation::Count(count),
            Message::Failed(error) => Observation::BackendError(error),
        };
        model.push(observed);
        Command::none()
    }
}

#[test]
fn shared_failure_message_preserves_domain_mapping_and_trace_reasons() {
    let cases = [
        (
            EffectOutcome::Succeeded("hello".to_owned()),
            EffectOutcome::Succeeded(5),
            vec![Observation::Text("hello".into()), Observation::Count(5)],
            [EffectOutcomeKind::Succeeded, EffectOutcomeKind::Succeeded],
        ),
        (
            EffectOutcome::Failed("read failed"),
            EffectOutcome::Failed("count failed"),
            vec![
                Observation::BackendError("read failed"),
                Observation::BackendError("count failed"),
            ],
            [EffectOutcomeKind::Failed, EffectOutcomeKind::Failed],
        ),
    ];

    for (text_outcome, count_outcome, expected, expected_trace) in cases {
        let (mut builder, capabilities) = Program::builder::<<Reader as Component>::Requires>();
        let reader = builder
            .component::<Reader>(())
            .name("reader")
            .with_capabilities(&capabilities)
            .finish()
            .unwrap();
        let mut runtime = ControlledRuntime::builder(builder.build().unwrap())
            .control_all()
            .build()
            .unwrap();
        runtime.send(&reader, Message::Read).unwrap();
        runtime.send(&reader, Message::Count).unwrap();
        runtime.run_until_idle().unwrap();
        let text = runtime.next_effect::<ReadText>().unwrap();
        let count = runtime.next_effect::<CountBytes>().unwrap();

        runtime.complete(text, text_outcome).unwrap();
        runtime.run_until_idle().unwrap();
        runtime.complete(count, count_outcome).unwrap();
        runtime.run_until_idle().unwrap();

        assert_eq!(runtime.state(&reader).unwrap(), &expected);
        let trace = runtime
            .trace()
            .iter()
            .filter_map(|record| match &record.event {
                TraceEvent::EffectOutcome { outcome, .. } => Some(*outcome),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(trace, expected_trace);
        assert_eq!(runtime.pending_work(), PendingWork::default());
        assert!(runtime.cancel().unwrap().is_clean());
    }
}

struct Stamper;

enum StampMessage {
    Stamp,
    Now(SystemTime),
}

impl Component for Stamper {
    type Requires = Effect<ReadSystemTime>;
    type Config = ();
    type Model = Option<SystemTime>;
    type Message = StampMessage;

    fn init(_: &(), _: &Capabilities<Self::Requires>) -> Init<Self::Model, StampMessage> {
        Init::default()
    }

    fn update(
        _: &(),
        clock: &Capabilities<Self::Requires>,
        model: &mut Self::Model,
        message: StampMessage,
    ) -> Command<StampMessage> {
        match message {
            StampMessage::Stamp => Effect::perform(clock, ReadSystemTime)
                .producing(|EffectOutcome::Succeeded(now)| StampMessage::Now(now)),
            StampMessage::Now(now) => {
                *model = Some(now);
                Command::none()
            }
        }
    }
}

#[test]
fn an_infallible_effect_takes_its_success_directly() {
    let (mut builder, clock) = Program::builder::<<Stamper as Component>::Requires>();
    let stamper = builder
        .component::<Stamper>(())
        .name("stamper")
        .with_capabilities(&clock)
        .finish()
        .unwrap();
    let mut runtime = ControlledRuntime::builder(builder.build().unwrap())
        .control_all()
        .build()
        .unwrap();
    runtime.send(&stamper, StampMessage::Stamp).unwrap();
    runtime.run_until_idle().unwrap();
    let pending = runtime.next_effect::<ReadSystemTime>().unwrap();

    let some_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_762_646_400);
    runtime
        .complete(pending, EffectOutcome::Succeeded(some_time))
        .unwrap();
    runtime.run_until_idle().unwrap();

    assert_eq!(runtime.state(&stamper).unwrap(), &Some(some_time));
    assert_eq!(runtime.pending_work(), PendingWork::default());
    assert!(runtime.cancel().unwrap().is_clean());
}
