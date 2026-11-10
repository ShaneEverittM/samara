//! ADR-0021: names are diagnostics; subscription keys are optional.
use samara::prelude::*;
use std::convert::Infallible;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Feed(u64);
impl SourceDescriptor for Feed {
    type Terminal = Self;
    type Item = u64;
    type Error = Infallible;
}

type Inputs = (Source<Feed>, Source<Feed>);
#[derive(Clone, Default)]
struct Model {
    desired: Vec<(u64, u64)>,
    keyed: bool,
    alternate: bool,
    offset: u64,
    received: Vec<u64>,
}
enum Message {
    Configure(Model),
    Input(u64),
}
struct Probe;
impl Component for Probe {
    type Requires = Inputs;
    type Config = Model;
    type Model = Model;
    type Message = Message;
    fn init(config: &Model, _: &Capabilities<Inputs>) -> Init<Model, Message> {
        Init::new(config.clone())
    }
    fn update(
        _: &Model,
        _: &Capabilities<Inputs>,
        model: &mut Model,
        message: Message,
    ) -> Command<Message> {
        match message {
            Message::Configure(next) => *model = next,
            Message::Input(value) => model.received.push(value),
        }
        Command::none()
    }
    fn subscriptions(
        _: &Model,
        caps: &Capabilities<Inputs>,
        model: &Model,
    ) -> Subscriptions<Message> {
        model
            .desired
            .iter()
            .map(|&(key, revision)| {
                let cap = if model.alternate { &caps.1 } else { &caps.0 };
                let offset = model.offset;
                let subscription = Subscription::source_with(cap, Feed(revision), move |event| {
                    Message::Input(match event {
                        SourceEvent::Item(value) => value + offset,
                        _ => 0,
                    })
                });
                if model.keyed {
                    subscription.key(key)
                } else {
                    subscription
                }
            })
            .collect::<Vec<_>>()
            .into()
    }
}

fn controlled(model: Model) -> (ControlledRuntime, ComponentRef<Probe>, Capabilities<Inputs>) {
    let (mut builder, caps) = Program::builder::<Inputs>();
    let probe = builder
        .component::<Probe>(model)
        .with_capabilities(caps.clone())
        .finish()
        .unwrap();
    let runtime = ControlledRuntime::builder(builder.build().unwrap())
        .control_all()
        .build()
        .unwrap();
    (runtime, probe, caps)
}
fn one(revision: u64) -> Model {
    Model {
        desired: vec![(1, revision)],
        ..Model::default()
    }
}
fn lifecycle(runtime: &ControlledRuntime, action: SubscriptionAction) -> usize {
    runtime
        .trace()
        .iter()
        .filter(|record| {
            matches!(&record.event,
                TraceEvent::SubscriptionLifecycle { action: observed, .. } if *observed == action
            )
        })
        .count()
}

#[test]
fn optional_and_duplicate_names_do_not_alias_components() {
    let (mut builder, caps) = Program::builder::<Inputs>();
    let first = builder
        .component::<Probe>(one(1))
        .name("same")
        .with_capabilities(caps.clone())
        .finish()
        .unwrap();
    let second = builder
        .component::<Probe>(one(1))
        .with_capabilities(caps.clone())
        .name("same")
        .finish()
        .unwrap();
    let unnamed = builder
        .component::<Probe>(Model::default())
        .with_capabilities(caps.clone())
        .finish()
        .unwrap();
    assert_ne!(first.id(), second.id());
    assert_eq!(first.id().name(), "same");
    assert_eq!(unnamed.id().name(), std::any::type_name::<Probe>());
    let mut runtime = ControlledRuntime::builder(builder.build().unwrap())
        .control_all()
        .build()
        .unwrap();
    runtime
        .emit_source::<_, Feed>(&first, &caps.0, SourceEvent::Item(7))
        .unwrap();
    runtime.send(&second, Message::Input(9)).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(runtime.state(&first).unwrap().received, [7]);
    assert_eq!(runtime.state(&second).unwrap().received, [9]);
    assert!(runtime.state(&unnamed).unwrap().received.is_empty());
}

#[test]
fn diagnostic_names_do_not_change_semantic_trace() {
    fn run(names: [&str; 2]) -> Vec<TraceRecord> {
        let (mut builder, caps) = Program::builder::<Inputs>();
        for name in names {
            builder
                .component::<Probe>(one(1))
                .name(name)
                .with_capabilities(caps.clone())
                .finish()
                .unwrap();
        }
        let mut runtime = ControlledRuntime::builder(builder.build().unwrap())
            .control_all()
            .build()
            .unwrap();
        runtime.run_until_idle().unwrap();
        runtime.trace().to_vec()
    }
    assert_eq!(run(["alpha", "beta"]), run(["zeta", "zeta"]));
}

#[test]
fn default_slot_retains_mapper_replaces_descriptor_and_cancels() {
    let (mut runtime, probe, caps) = controlled(one(1));
    runtime.run_until_idle().unwrap();
    assert_eq!(lifecycle(&runtime, SubscriptionAction::Started), 1);
    let mut model = one(1);
    model.offset = 10;
    runtime.send(&probe, Message::Configure(model)).unwrap();
    runtime.run_until_idle().unwrap();
    runtime
        .emit_source::<_, Feed>(&probe, &caps.0, SourceEvent::Item(2))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(runtime.state(&probe).unwrap().received, [12]);
    assert_eq!(lifecycle(&runtime, SubscriptionAction::Started), 1);
    assert_eq!(lifecycle(&runtime, SubscriptionAction::Replaced), 0);
    runtime.send(&probe, Message::Configure(one(2))).unwrap();
    // Accepted old-generation work must be discarded when replacement commits.
    runtime
        .emit_source::<_, Feed>(&probe, &caps.0, SourceEvent::Item(99))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.state(&probe).unwrap().received.is_empty());
    assert_eq!(
        runtime
            .source_descriptor::<_, Feed>(&probe, &caps.0)
            .unwrap(),
        Feed(2)
    );
    assert_eq!(lifecycle(&runtime, SubscriptionAction::Replaced), 1);
    runtime
        .send(&probe, Message::Configure(Model::default()))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(
        runtime
            .emit_source::<_, Feed>(&probe, &caps.0, SourceEvent::Item(3))
            .is_err()
    );
    assert_eq!(lifecycle(&runtime, SubscriptionAction::Cancelled), 1);
}

#[test]
fn ambiguous_defaults_explain_how_to_supply_keys() {
    let (mut builder, caps) = Program::builder::<Inputs>();
    builder
        .component::<Probe>(Model {
            desired: vec![(1, 1), (2, 2)],
            ..Model::default()
        })
        .with_capabilities(caps)
        .finish()
        .unwrap();
    let error = ControlledRuntime::builder(builder.build().unwrap())
        .control_all()
        .build()
        .err()
        .unwrap();
    assert!(error.to_string().contains("key("), "{error}");
}

#[test]
fn keyed_collection_survives_reordering_and_capability_replacement() {
    let model = Model {
        desired: vec![(1, 10), (2, 20)],
        keyed: true,
        ..Model::default()
    };
    let (mut runtime, probe, _) = controlled(model.clone());
    runtime.run_until_idle().unwrap();
    let mut reordered = model.clone();
    reordered.desired.reverse();
    runtime.send(&probe, Message::Configure(reordered)).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(lifecycle(&runtime, SubscriptionAction::Started), 2);
    assert_eq!(lifecycle(&runtime, SubscriptionAction::Replaced), 0);
    runtime
        .emit_source::<_, Feed>(&probe, SubscriptionId::key(2_u64), SourceEvent::Item(8))
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(runtime.state(&probe).unwrap().received, [8]);
    let mut changed = model;
    changed.alternate = true;
    runtime.send(&probe, Message::Configure(changed)).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(lifecycle(&runtime, SubscriptionAction::Replaced), 2);
}

#[test]
fn key_identity_preserves_type_and_value_not_debug_output() {
    #[derive(PartialEq, Eq, Hash)]
    struct Key(u64);
    impl std::fmt::Debug for Key {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("same")
        }
    }
    assert_eq!(SubscriptionId::key(Key(1)), SubscriptionId::key(Key(1)));
    assert_ne!(SubscriptionId::key(Key(1)), SubscriptionId::key(Key(2)));
    assert_ne!(SubscriptionId::key(1_u64), SubscriptionId::key(1_u32));
}

#[test]
fn foreign_capability_cannot_select_a_default_slot() {
    let (mut runtime, probe, _) = controlled(one(1));
    let (_, foreign) = Program::builder::<Inputs>();
    assert!(
        runtime
            .emit_source::<_, Feed>(&probe, &foreign.0, SourceEvent::Item(7))
            .is_err()
    );
    assert!(
        runtime
            .source_descriptor::<_, Feed>(&probe, &foreign.0)
            .is_err()
    );
}

#[test]
fn default_slots_distinguish_exact_capabilities_and_explicit_keys() {
    let (_, caps) = Program::builder::<Inputs>();
    let first = Subscription::<SourceEvent<u64, Infallible>>::source(&caps.0, Feed(1));
    let second = Subscription::<SourceEvent<u64, Infallible>>::source(&caps.1, Feed(1));
    let keyed = Subscription::<SourceEvent<u64, Infallible>>::source(&caps.0, Feed(1)).key(0_u64);
    assert_ne!(first.id(), second.id());
    assert_ne!(first.id(), keyed.id());
    let inputs: Subscriptions<_> = vec![first, second, keyed].into();
    assert_eq!(inputs.find::<Feed>(&caps.1), Some(&Feed(1)));
    let (_, foreign) = Program::builder::<Inputs>();
    assert!(inputs.find::<Feed>(&foreign.1).is_none());
}

#[test]
fn dropped_registration_does_not_change_identity_or_startup_trace() {
    fn run(drop_registration: bool) -> Vec<TraceRecord> {
        let (mut builder, caps) = Program::builder::<Inputs>();
        if drop_registration {
            drop(builder.component::<Probe>(one(99)).name("discarded"));
        }
        builder
            .component::<Probe>(one(1))
            .with_capabilities(caps)
            .finish()
            .unwrap();
        let runtime = ControlledRuntime::builder(builder.build().unwrap())
            .control_all()
            .build()
            .unwrap();
        runtime.trace().to_vec()
    }
    assert_eq!(run(false), run(true));
}

struct Report(u64);
impl EffectDescriptor for Report {
    type Output = ();
    type Error = Infallible;
}
type MirrorProgram = Program<(Source<Feed>, Effect<Report>)>;
struct Mirror;
impl Component for Mirror {
    type Requires = (Source<Feed>, Effect<Report>);
    type Config = ();
    type Model = ();
    type Message = SourceEvent<u64, Infallible>;
    fn init(_: &(), _: &Capabilities<Self::Requires>) -> Init<(), Self::Message> {
        Init::new(())
    }
    fn update(
        _: &(),
        caps: &Capabilities<Self::Requires>,
        _: &mut (),
        event: Self::Message,
    ) -> Command<Self::Message> {
        match event {
            SourceEvent::Item(value) => Command::effect_discarding_outcome(&caps.1, Report(value)),
            _ => Command::none(),
        }
    }
    fn subscriptions(
        _: &(),
        caps: &Capabilities<Self::Requires>,
        _: &(),
    ) -> Subscriptions<Self::Message> {
        Subscriptions::one(Subscription::source(&caps.0, Feed(7)))
    }
}
struct FeedDriver;
impl SourceDriver<Feed> for FeedDriver {
    fn run(&self, feed: Feed, sink: SourceSink<Feed>) -> BoxFuture<()> {
        Box::pin(async move {
            let _ = sink.emit(feed.0).await;
            let _ = sink.end().await;
        })
    }
}
struct ReportDriver(tokio::sync::mpsc::UnboundedSender<u64>);
impl EffectDriver<Report> for ReportDriver {
    fn execute(&self, report: Report) -> BoxFuture<Result<(), Infallible>> {
        let sender = self.0.clone();
        Box::pin(async move {
            sender.send(report.0).unwrap();
            Ok(())
        })
    }
}

#[tokio::test]
async fn default_subscription_runs_unchanged_live_and_controlled() {
    fn program() -> (MirrorProgram, ComponentRef<Mirror>, SourceCapability<Feed>) {
        let (mut builder, caps) = Program::builder::<(Source<Feed>, Effect<Report>)>();
        let source = caps.0.clone();
        let mirror = builder
            .component::<Mirror>(())
            .with_capabilities(caps)
            .finish()
            .unwrap();
        (builder.build().unwrap(), mirror, source)
    }
    let (definition, mirror, source) = program();
    let mut controlled = ControlledRuntime::builder(definition)
        .control_all()
        .build()
        .unwrap();
    controlled
        .emit_source::<_, Feed>(&mirror, &source, SourceEvent::Item(7))
        .unwrap();
    controlled.run_until_idle().unwrap();
    let report = controlled.next_effect::<Report>().unwrap();
    let expected = report.intent.0;
    controlled
        .complete(report, EffectOutcome::Succeeded(()))
        .unwrap();
    assert!(controlled.cancel().unwrap().is_clean());

    let (definition, _, _) = program();
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let runtime = Runtime::builder(definition)
        .with_drivers((DrivenSource::new(FeedDriver), ReportDriver(sender)))
        .build()
        .unwrap()
        .spawn();
    assert_eq!(receiver.recv().await.unwrap(), expected);
    assert!(runtime.shutdown(Shutdown::Drain).await.unwrap().is_clean());
}
