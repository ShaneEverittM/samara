//! ADR-0020 acceptance: success-specific Messages and one failure-only Message
//! keep routing separate from domain mapping, without losing trace reasons.

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
    Cancelled(CancelReason),
}

enum Message {
    Read,
    Count,
    Text(String),
    Counted(usize),
    Failed(EffectFailure<&'static str>),
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
                return Command::effect_with(read, ReadText, |outcome| match outcome {
                    EffectOutcome::Succeeded(text) => Message::Text(text),
                    EffectOutcome::Failed(reason) => Message::Failed(reason),
                });
            }
            Message::Count => {
                return Command::effect_with(count, CountBytes, |outcome| match outcome {
                    EffectOutcome::Succeeded(count) => Message::Counted(count),
                    EffectOutcome::Failed(reason) => Message::Failed(reason),
                });
            }
            Message::Text(text) => Observation::Text(text),
            Message::Counted(count) => Observation::Count(count),
            Message::Failed(reason) => match reason {
                EffectFailure::Error(error) => Observation::BackendError(error),
                EffectFailure::Cancelled(reason) => Observation::Cancelled(reason),
            },
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
            EffectOutcome::Failed(EffectFailure::Error("read failed")),
            EffectOutcome::Failed(EffectFailure::Cancelled(CancelReason::Superseded)),
            vec![
                Observation::BackendError("read failed"),
                Observation::Cancelled(CancelReason::Superseded),
            ],
            [EffectOutcomeKind::Failed, EffectOutcomeKind::Cancelled],
        ),
        (
            EffectOutcome::Failed(EffectFailure::Cancelled(CancelReason::Deadline)),
            EffectOutcome::Failed(EffectFailure::Error("count failed")),
            vec![
                Observation::Cancelled(CancelReason::Deadline),
                Observation::BackendError("count failed"),
            ],
            [EffectOutcomeKind::Cancelled, EffectOutcomeKind::Failed],
        ),
    ];

    for (text_outcome, count_outcome, expected, expected_trace) in cases {
        let (mut builder, capabilities) = Program::builder::<<Reader as Component>::Requires>();
        let reader = builder
            .component::<Reader>(ComponentId::new("reader"), ())
            .with_capabilities(capabilities)
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
