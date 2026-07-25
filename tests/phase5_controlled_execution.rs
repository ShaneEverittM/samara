//! Executable acceptance evidence for the Phase 5 controlled runtime.
//!
//! Scenario names trace directly to ADR-0003 and
//! `docs/testing/v0-acceptance-matrix.md`. The assertions stay at semantic
//! observation points: Component state, typed intent, logical time, causal
//! trace records, Source lifecycle, and pending-work ownership. They do not
//! inspect queues, tasks, locks, or another runtime-topology choice.

use std::{
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use samara::prelude::*;

// Program assembly ---------------------------------------------------------

struct Empty;

impl Component for Empty {
    type Model = ();
    type Message = ();

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(())
    }

    fn update(&self, _model: &mut Self::Model, _message: Self::Message) -> Command<Self::Message> {
        Command::none()
    }
}

struct AssemblyProtocol;

impl Protocol for AssemblyProtocol {
    type Message = ();
}

#[test]
fn phase5_program_build_rejects_duplicate_component_id() {
    let mut builder = Program::builder();
    let _ = builder.component(ComponentId::new("duplicate"), Empty);
    let _ = builder.component(ComponentId::new("duplicate"), Empty);

    assert!(builder.build().is_err());
}

#[test]
fn phase5_program_build_rejects_duplicate_protocol_and_port_id() {
    let mut builder = Program::builder();
    let _ = builder.port::<AssemblyProtocol>(PortId::new("service"));
    let _ = builder.port::<AssemblyProtocol>(PortId::new("service"));

    assert!(builder.build().is_err());
}

#[test]
fn phase5_program_build_requires_exactly_one_binding_per_port() {
    let mut unbound = Program::builder();
    let _ = unbound.port::<AssemblyProtocol>(PortId::new("service"));
    assert!(unbound.build().is_err());

    let mut duplicate = Program::builder();
    let port = duplicate.port::<AssemblyProtocol>(PortId::new("service"));
    let provider = duplicate.component(ComponentId::new("provider"), Empty);
    duplicate.bind_port(&port, &provider);
    duplicate.bind_port(&port, &provider);
    assert!(duplicate.build().is_err());
}

#[test]
fn phase5_program_build_rejects_provider_from_another_builder() {
    let mut provider_builder = Program::builder();
    let provider = provider_builder.component(ComponentId::new("provider"), Empty);

    let mut consumer_builder = Program::builder();
    let port = consumer_builder.port::<AssemblyProtocol>(PortId::new("service"));
    consumer_builder.bind_port(&port, &provider);

    assert!(consumer_builder.build().is_err());
}

struct LeftProtocol;
struct RightProtocol;

impl Protocol for LeftProtocol {
    type Message = ();
}

impl Protocol for RightProtocol {
    type Message = ();
}

struct Left {
    _right: Port<RightProtocol>,
}

struct Right {
    _left: Port<LeftProtocol>,
}

impl Component for Left {
    type Model = ();
    type Message = ();

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(())
    }

    fn update(&self, _model: &mut Self::Model, _message: Self::Message) -> Command<Self::Message> {
        Command::none()
    }
}

impl Component for Right {
    type Model = ();
    type Message = ();

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(())
    }

    fn update(&self, _model: &mut Self::Model, _message: Self::Message) -> Command<Self::Message> {
        Command::none()
    }
}

#[test]
fn phase5_program_build_allows_port_cycles() {
    let mut builder = Program::builder();
    let left_port = builder.port::<LeftProtocol>(PortId::new("left"));
    let right_port = builder.port::<RightProtocol>(PortId::new("right"));
    let left = builder.component(
        ComponentId::new("left"),
        Left {
            _right: right_port.clone(),
        },
    );
    let right = builder.component(
        ComponentId::new("right"),
        Right {
            _left: left_port.clone(),
        },
    );
    builder.bind_port(&left_port, &left);
    builder.bind_port(&right_port, &right);

    assert!(builder.build().is_ok());
}

// Effects, faults, and semantic ownership ---------------------------------

#[derive(Debug, PartialEq, Eq)]
struct ProbeEffect(u64);

impl EffectDescriptor for ProbeEffect {
    type Output = u64;
    type Error = Infallible;
}

#[derive(Debug)]
enum EffectMessage {
    Start,
    Finished(EffectOutcome<u64, Infallible>),
}

#[derive(Debug, Default, PartialEq, Eq)]
struct EffectModel {
    starts: usize,
    output: Option<u64>,
}

struct EffectComponent {
    mapper_calls: Arc<AtomicUsize>,
}

impl Component for EffectComponent {
    type Model = EffectModel;
    type Message = EffectMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(EffectModel::default())
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            EffectMessage::Start => {
                model.starts += 1;
                let mapper_calls = self.mapper_calls.clone();
                Command::effect_with(ProbeEffect(41), move |outcome| {
                    mapper_calls.fetch_add(1, Ordering::SeqCst);
                    EffectMessage::Finished(outcome)
                })
            }
            EffectMessage::Finished(EffectOutcome::Succeeded(output)) => {
                model.output = Some(output);
                Command::none()
            }
            EffectMessage::Finished(EffectOutcome::Failed(never)) => match never {},
            EffectMessage::Finished(EffectOutcome::Cancelled(_)) => Command::none(),
        }
    }
}

fn effect_program(mapper_calls: Arc<AtomicUsize>) -> (Program, ComponentRef<EffectComponent>) {
    let mut builder = Program::builder();
    let component = builder.component(ComponentId::new("effect"), EffectComponent { mapper_calls });
    (builder.build().expect("valid effect program"), component)
}

#[test]
fn v3_controlled_effect_maps_exactly_once_without_live_driver() {
    let mapper_calls = Arc::new(AtomicUsize::new(0));
    let (program, component) = effect_program(mapper_calls.clone());
    let mut runtime = ControlledRuntime::builder(program)
        .control_effect::<ProbeEffect>()
        .build()
        .expect("controlled effect binding");

    runtime
        .send(&component, EffectMessage::Start)
        .expect("controlled input");
    let report = runtime.run_until_idle().expect("effect is intercepted");
    assert_eq!(report.pending_now, 0);
    assert_eq!(report.pending_later, 1);
    assert_eq!(mapper_calls.load(Ordering::SeqCst), 0);

    let pending = runtime
        .next_effect::<ProbeEffect>()
        .expect("typed effect occurrence");
    assert_eq!(pending.intent, ProbeEffect(41));
    runtime
        .complete(pending, EffectOutcome::Succeeded(42))
        .expect("scripted outcome");
    runtime.run_until_idle().expect("mapped message re-enters");

    assert_eq!(mapper_calls.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.state(&component).unwrap().output, Some(42));
    assert_eq!(runtime.pending_work(), PendingWork::default());

    let effect = runtime
        .trace()
        .iter()
        .find(|record| {
            matches!(
                record.event,
                TraceEvent::CommandEmitted {
                    kind: TraceCommandKind::Effect,
                    ..
                }
            )
        })
        .expect("effect command trace");
    let outcome = runtime
        .trace()
        .iter()
        .find(|record| matches!(record.event, TraceEvent::EffectOutcome { .. }))
        .expect("scripted effect outcome trace");
    assert_eq!(outcome.cause, None, "controlled outcomes are harness roots");
    assert!(matches!(
        outcome.event,
        TraceEvent::EffectOutcome {
            effect: request,
            ..
        } if request == effect.id
    ));
    assert!(runtime.trace().iter().any(|record| {
        record.cause == Some(outcome.id) && matches!(record.event, TraceEvent::Transition { .. })
    }));
}

#[test]
fn v9_effect_cancellation_has_one_outcome() {
    let mapper_calls = Arc::new(AtomicUsize::new(0));
    let (program, component) = effect_program(mapper_calls.clone());
    let mut runtime = ControlledRuntime::builder(program)
        .control_effect::<ProbeEffect>()
        .build()
        .unwrap();
    runtime.send(&component, EffectMessage::Start).unwrap();
    runtime.run_until_idle().unwrap();
    let pending = runtime.next_effect::<ProbeEffect>().unwrap();

    runtime
        .complete(pending, EffectOutcome::Cancelled(CancelReason::Shutdown))
        .unwrap();
    runtime.run_until_idle().unwrap();

    assert_eq!(mapper_calls.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.pending_work(), PendingWork::default());
    assert_eq!(
        runtime
            .trace()
            .iter()
            .filter(|record| matches!(
                record.event,
                TraceEvent::EffectOutcome {
                    outcome: EffectOutcomeKind::Cancelled,
                    ..
                }
            ))
            .count(),
        1
    );
}

#[test]
fn phase5_missing_controlled_behavior_faults_without_live_fallback() {
    let mapper_calls = Arc::new(AtomicUsize::new(0));
    let (program, component) = effect_program(mapper_calls.clone());
    let mut runtime = ControlledRuntime::builder(program)
        .build()
        .expect("the logical program is valid");

    runtime.send(&component, EffectMessage::Start).unwrap();
    let error = runtime
        .run_until_idle()
        .expect_err("an unbound terminal effect must fault");

    assert_eq!(error.component(), Some(component.id()));
    assert_eq!(
        error.descriptor_type(),
        Some(std::any::type_name::<ProbeEffect>())
    );
    assert!(error.work_occurrence().is_some());
    assert_eq!(mapper_calls.load(Ordering::SeqCst), 0);
    assert_eq!(runtime.state(&component).unwrap().starts, 1);
    assert!(runtime.trace().iter().any(|record| matches!(
        &record.event,
        TraceEvent::RuntimeFault {
            descriptor_type,
            ..
        } if *descriptor_type == std::any::type_name::<ProbeEffect>()
    )));
}

#[test]
fn phase5_faulted_run_remains_inspectable_and_cancellable() {
    let (program, component) = effect_program(Arc::new(AtomicUsize::new(0)));
    let mut runtime = ControlledRuntime::builder(program).build().unwrap();
    runtime.send(&component, EffectMessage::Start).unwrap();
    let first = runtime.run_until_idle().expect_err("first drive faults");

    assert_eq!(runtime.state(&component).unwrap().starts, 1);
    assert!(!runtime.trace().is_empty());
    assert_eq!(runtime.run_until_idle().unwrap_err(), first);

    let report = runtime.cancel().expect("faulted work remains cancellable");
    assert!(report.is_clean());
    assert_eq!(report.pending_now, 0);
    assert_eq!(report.pending_later, 0);
}

// Subscription retention, cutover, and SourcePlan lowering -----------------

#[derive(Debug, Default, PartialEq, Eq)]
struct MappingModel {
    mapper_version: u8,
    binding: &'static str,
    observed: Vec<(u8, u64)>,
}

#[derive(Debug)]
enum MappingMessage {
    UseMapper(u8),
    Replace(&'static str),
    Observed { mapper: u8, value: u64 },
    Ended,
}

struct MappingComponent;

impl Component for MappingComponent {
    type Model = MappingModel;
    type Message = MappingMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(MappingModel {
            mapper_version: 1,
            binding: "one",
            observed: Vec::new(),
        })
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            MappingMessage::UseMapper(version) => model.mapper_version = version,
            MappingMessage::Replace(binding) => model.binding = binding,
            MappingMessage::Observed { mapper, value } => model.observed.push((mapper, value)),
            MappingMessage::Ended => {}
        }
        Command::none()
    }

    fn subscriptions(&self, model: &Self::Model) -> Subscriptions<Self::Message> {
        let mapper = model.mapper_version;
        Subscriptions::one(Subscription::source_with(
            SubscriptionId::new("input"),
            StreamDescriptor::<u64>::named(model.binding),
            move |event| match event {
                SourceEvent::Item(value) => MappingMessage::Observed { mapper, value },
                SourceEvent::Ended => MappingMessage::Ended,
                SourceEvent::Failed(never) => match never {},
            },
        ))
    }
}

fn mapping_program() -> (Program, ComponentRef<MappingComponent>) {
    let mut builder = Program::builder();
    let component = builder.component(ComponentId::new("mapping"), MappingComponent);
    (builder.build().expect("valid source program"), component)
}

#[test]
fn v4_equal_descriptor_retains_source_and_adopts_latest_mapper() {
    let (program, component) = mapping_program();
    let mut runtime = ControlledRuntime::builder(program)
        .control_source::<StreamDescriptor<u64>>()
        .build()
        .unwrap();

    runtime
        .send(&component, MappingMessage::UseMapper(2))
        .unwrap();
    runtime
        .emit_source::<MappingComponent, StreamDescriptor<u64>>(
            &component,
            &SubscriptionId::new("input"),
            SourceEvent::Item(7),
        )
        .unwrap();
    runtime.run_until_idle().unwrap();

    assert_eq!(runtime.state(&component).unwrap().observed, vec![(2, 7)]);
    let starts = runtime
        .trace()
        .iter()
        .filter(|record| {
            matches!(
                record.event,
                TraceEvent::SubscriptionLifecycle {
                    action: SubscriptionAction::Started,
                    ..
                }
            )
        })
        .count();
    assert_eq!(starts, 1, "retention must not restart the Source");
}

#[test]
fn v4_terminal_source_event_ends_the_active_realization() {
    let (program, component) = mapping_program();
    let mut runtime = ControlledRuntime::builder(program)
        .control_source::<StreamDescriptor<u64>>()
        .build()
        .unwrap();
    runtime
        .emit_source::<MappingComponent, StreamDescriptor<u64>>(
            &component,
            &SubscriptionId::new("input"),
            SourceEvent::Ended,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();

    assert_eq!(runtime.pending_work(), PendingWork::default());
    assert!(
        runtime
            .source_descriptor::<MappingComponent, StreamDescriptor<u64>>(
                &component,
                &SubscriptionId::new("input")
            )
            .is_err(),
        "a terminal event must not leave an inspectable active Source"
    );
}

#[test]
fn v4_already_mapped_message_keeps_its_original_meaning_after_retention() {
    let (program, component) = mapping_program();
    let mut runtime = ControlledRuntime::builder(program)
        .control_source::<StreamDescriptor<u64>>()
        .build()
        .unwrap();

    // The event maps first and queues a version-1 Message. Reconciliation then
    // installs mapper version 2 without changing the generation; the already
    // created Message retains version 1 when its transition begins.
    runtime
        .emit_source::<MappingComponent, StreamDescriptor<u64>>(
            &component,
            &SubscriptionId::new("input"),
            SourceEvent::Item(7),
        )
        .unwrap();
    runtime
        .send(&component, MappingMessage::UseMapper(2))
        .unwrap();
    runtime.run_until_idle().unwrap();

    assert_eq!(runtime.state(&component).unwrap().observed, vec![(1, 7)]);
}

#[test]
fn v4_replaced_generation_discards_stale_work() {
    let (program, component) = mapping_program();
    let mut runtime = ControlledRuntime::builder(program)
        .control_source::<StreamDescriptor<u64>>()
        .build()
        .unwrap();

    // The replacement message receives the earlier insertion ticket. The old
    // generation's still-queued event is rejected before it reaches a mapper.
    runtime
        .send(&component, MappingMessage::Replace("two"))
        .unwrap();
    runtime
        .emit_source::<MappingComponent, StreamDescriptor<u64>>(
            &component,
            &SubscriptionId::new("input"),
            SourceEvent::Item(9),
        )
        .unwrap();
    runtime.run_until_idle().unwrap();

    assert!(runtime.state(&component).unwrap().observed.is_empty());
    assert_eq!(
        runtime
            .source_descriptor::<MappingComponent, StreamDescriptor<u64>>(
                &component,
                &SubscriptionId::new("input")
            )
            .unwrap(),
        StreamDescriptor::named("two")
    );
    assert!(runtime.trace().iter().any(|record| matches!(
        record.event,
        TraceEvent::StaleSourceWorkDropped {
            mapped_message: false,
            ..
        }
    )));
}

#[test]
fn v4_replaced_generation_discards_already_mapped_message() {
    let (program, component) = mapping_program();
    let mut runtime = ControlledRuntime::builder(program)
        .control_source::<StreamDescriptor<u64>>()
        .build()
        .unwrap();

    // The event maps first and queues an old-generation Message. The already
    // accepted replacement then commits before that Message's transition.
    runtime
        .emit_source::<MappingComponent, StreamDescriptor<u64>>(
            &component,
            &SubscriptionId::new("input"),
            SourceEvent::Item(11),
        )
        .unwrap();
    runtime
        .send(&component, MappingMessage::Replace("two"))
        .unwrap();
    runtime.run_until_idle().unwrap();

    assert!(runtime.state(&component).unwrap().observed.is_empty());
    assert!(runtime.trace().iter().any(|record| matches!(
        record.event,
        TraceEvent::StaleSourceWorkDropped {
            mapped_message: true,
            ..
        }
    )));
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RawBytes;

impl SourceDescriptor for RawBytes {
    type Item = Vec<u8>;
    type Error = Infallible;
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LengthPrefix;

#[derive(Default)]
struct LengthState(Vec<u8>);

impl Decoder for LengthPrefix {
    type Chunk = Vec<u8>;
    type Frame = Vec<u8>;
    type Error = Infallible;
    type State = LengthState;

    fn start(&self) -> Self::State {
        LengthState::default()
    }

    fn push(
        &self,
        state: &mut Self::State,
        chunk: Self::Chunk,
    ) -> Result<Vec<Self::Frame>, Self::Error> {
        state.0.extend(chunk);
        let mut frames = Vec::new();
        while let Some((&length, payload)) = state.0.split_first() {
            let length = usize::from(length);
            if payload.len() < length {
                break;
            }
            frames.push(payload[..length].to_vec());
            state.0.drain(..=length);
        }
        Ok(frames)
    }

    fn finish(&self, state: &mut Self::State) -> Result<Vec<Self::Frame>, Self::Error> {
        Ok((!state.0.is_empty())
            .then(|| std::mem::take(&mut state.0))
            .into_iter()
            .collect())
    }
}

type TwiceFramed = Framed<Framed<RawBytes, LengthPrefix>, LengthPrefix>;

#[derive(Debug)]
enum FramedMessage {
    Event(SourceEvent<Vec<u8>, FramedError<FramedError<Infallible, Infallible>, Infallible>>),
}

#[derive(Default, Debug, PartialEq, Eq)]
struct FramedModel(Vec<Vec<u8>>);

struct FramedComponent;

impl Component for FramedComponent {
    type Model = FramedModel;
    type Message = FramedMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(FramedModel::default())
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        let FramedMessage::Event(event) = message;
        if let SourceEvent::Item(frame) = event {
            model.0.push(frame);
        }
        Command::none()
    }

    fn subscriptions(&self, _model: &Self::Model) -> Subscriptions<Self::Message> {
        Subscriptions::one(Subscription::source_with(
            SubscriptionId::new("framed"),
            Framed::new(Framed::new(RawBytes, LengthPrefix), LengthPrefix),
            FramedMessage::Event,
        ))
    }
}

#[test]
fn v4_composed_source_plan_reaches_terminal_controlled_behavior() {
    let mut builder = Program::builder();
    let component = builder.component(ComponentId::new("framed"), FramedComponent);
    let program = builder.build().unwrap();
    let mut runtime = ControlledRuntime::builder(program)
        .control_source::<RawBytes>()
        .build()
        .unwrap();

    runtime
        .emit_source::<FramedComponent, RawBytes>(
            &component,
            &SubscriptionId::new("framed"),
            SourceEvent::Item(vec![3, 2, b'a', b'b']),
        )
        .unwrap();
    runtime.run_until_idle().unwrap();

    assert_eq!(runtime.state(&component).unwrap().0, vec![b"ab".to_vec()]);
    assert_eq!(
        runtime
            .source_descriptor::<FramedComponent, TwiceFramed>(
                &component,
                &SubscriptionId::new("framed")
            )
            .unwrap(),
        Framed::new(Framed::new(RawBytes, LengthPrefix), LengthPrefix)
    );
}

#[test]
fn v4_composed_source_binding_must_name_the_terminal_descriptor() {
    let mut builder = Program::builder();
    let component = builder.component(ComponentId::new("framed"), FramedComponent);
    let program = builder.build().unwrap();
    let mut runtime = ControlledRuntime::builder(program)
        .control_source::<TwiceFramed>()
        .build()
        .unwrap();

    let error = runtime
        .run_until_idle()
        .expect_err("binding the composed type must not hide a missing terminal binding");
    assert_eq!(error.component(), Some(component.id()));
    assert_eq!(
        error.descriptor_type(),
        Some(std::any::type_name::<RawBytes>())
    );
    assert!(runtime.trace().iter().any(|record| matches!(
        record.event,
        TraceEvent::RuntimeFault {
            descriptor_type,
            ..
        } if descriptor_type == std::any::type_name::<RawBytes>()
    )));
    assert!(runtime.cancel().unwrap().is_clean());
}

// Logical time, determinism, trace causality -------------------------------

#[derive(Clone, Debug)]
enum TimerMessage {
    Mark(&'static str),
}

#[derive(Default, Debug, PartialEq, Eq)]
struct TimerModel(Vec<&'static str>);

struct TimerComponent;

impl Component for TimerComponent {
    type Model = TimerModel;
    type Message = TimerMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(TimerModel::default()).with_command(Command::batch([
            Command::after(Duration::from_millis(5), TimerMessage::Mark("first")),
            Command::after(Duration::from_millis(5), TimerMessage::Mark("second")),
        ]))
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        let TimerMessage::Mark(value) = message;
        model.0.push(value);
        Command::none()
    }
}

fn timer_program(
    reverse_registration: bool,
) -> (
    Program,
    ComponentRef<TimerComponent>,
    ComponentRef<TimerComponent>,
) {
    let mut builder = Program::builder();
    let (alpha, beta) = if reverse_registration {
        let beta = builder.component(ComponentId::new("beta"), TimerComponent);
        let alpha = builder.component(ComponentId::new("alpha"), TimerComponent);
        (alpha, beta)
    } else {
        let alpha = builder.component(ComponentId::new("alpha"), TimerComponent);
        let beta = builder.component(ComponentId::new("beta"), TimerComponent);
        (alpha, beta)
    };
    (builder.build().unwrap(), alpha, beta)
}

fn run_timers(
    reverse_registration: bool,
    automatic: bool,
) -> (Vec<TraceRecord>, TimerModel, TimerModel) {
    let (program, alpha, beta) = timer_program(reverse_registration);
    let mut runtime = ControlledRuntime::builder(program).build().unwrap();
    assert_eq!(runtime.pending_work().pending_later, 4);
    if automatic {
        runtime.advance_to_next().unwrap();
    } else {
        runtime.advance(Duration::from_millis(5)).unwrap();
    }
    (
        runtime.trace().to_vec(),
        TimerModel(runtime.state(&alpha).unwrap().0.clone()),
        TimerModel(runtime.state(&beta).unwrap().0.clone()),
    )
}

#[test]
fn v7_equal_time_uses_deterministic_causal_insertion_order() {
    let (_, alpha, beta) = run_timers(false, false);
    assert_eq!(alpha.0, vec!["first", "second"]);
    assert_eq!(beta.0, vec!["first", "second"]);
}

#[test]
fn v7_initial_work_uses_component_id_not_registration_order() {
    let normal = run_timers(false, false);
    let reversed = run_timers(true, false);
    assert_eq!(normal, reversed);
}

#[test]
fn v7_manual_and_automatic_advance_are_repeatable_without_wall_sleep() {
    assert_eq!(run_timers(false, false), run_timers(false, true));
}

#[derive(Debug)]
enum CausalTimerMessage {
    Start,
    Tick,
}

struct CausalTimer;

impl Component for CausalTimer {
    type Model = Vec<&'static str>;
    type Message = CausalTimerMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(Vec::new())
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            CausalTimerMessage::Start => {
                model.push("start");
                Command::after(Duration::from_millis(5), CausalTimerMessage::Tick)
            }
            CausalTimerMessage::Tick => {
                model.push("tick");
                Command::none()
            }
        }
    }
}

fn run_causal_timer(automatic: bool) -> (Vec<&'static str>, Vec<TraceRecord>) {
    let mut builder = Program::builder();
    let component = builder.component(ComponentId::new("causal-timer"), CausalTimer);
    let program = builder.build().unwrap();
    let mut runtime = ControlledRuntime::builder(program).build().unwrap();
    runtime.send(&component, CausalTimerMessage::Start).unwrap();
    if automatic {
        runtime.advance_to_next().unwrap();
    } else {
        runtime.advance(Duration::from_millis(5)).unwrap();
    }
    (
        runtime.state(&component).unwrap().clone(),
        runtime.trace().to_vec(),
    )
}

#[test]
fn v7_manual_advance_drains_due_causes_before_moving_logical_time() {
    let manual = run_causal_timer(false);
    let automatic = run_causal_timer(true);

    assert_eq!(manual, automatic);
    assert_eq!(manual.0, vec!["start", "tick"]);
}

#[test]
fn phase5_logical_time_overflow_is_a_harness_error_not_a_panic() {
    let program = Program::builder().build().unwrap();
    let mut runtime = ControlledRuntime::builder(program).build().unwrap();
    runtime.advance(Duration::from_nanos(1)).unwrap();

    assert!(runtime.advance(Duration::MAX).is_err());
    assert!(runtime.run_until_idle().is_ok());
}

#[derive(Debug)]
enum OverflowTimerMessage {
    Arm,
    Fire,
}

struct OverflowTimer;

impl Component for OverflowTimer {
    type Model = bool;
    type Message = OverflowTimerMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(false)
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            OverflowTimerMessage::Arm => Command::after(Duration::MAX, OverflowTimerMessage::Fire),
            OverflowTimerMessage::Fire => {
                *model = true;
                Command::none()
            }
        }
    }
}

#[test]
fn phase5_timer_deadline_overflow_faults_instead_of_panicking() {
    let mut builder = Program::builder();
    let component = builder.component(ComponentId::new("overflow"), OverflowTimer);
    let program = builder.build().unwrap();
    let mut runtime = ControlledRuntime::builder(program).build().unwrap();
    runtime.advance(Duration::from_nanos(1)).unwrap();
    runtime.send(&component, OverflowTimerMessage::Arm).unwrap();

    let error = runtime.run_until_idle().expect_err("the timer must fault");
    assert_eq!(error.component(), Some(component.id()));
    assert!(!*runtime.state(&component).unwrap());
    assert!(runtime.cancel().unwrap().is_clean());
}

#[test]
fn v6_identical_controlled_runs_have_equal_structural_trace_and_final_state() {
    assert_eq!(run_timers(false, false), run_timers(false, false));
}

#[test]
fn v6_equal_final_state_with_different_structural_trace_is_not_equivalent() {
    struct Balance;

    impl Component for Balance {
        type Model = i32;
        type Message = i32;

        fn init(&self) -> Init<Self::Model, Self::Message> {
            Init::new(0)
        }

        fn update(
            &self,
            model: &mut Self::Model,
            message: Self::Message,
        ) -> Command<Self::Message> {
            *model += message;
            Command::none()
        }
    }

    fn run(inputs: &[i32]) -> (i32, Vec<TraceRecord>) {
        let mut builder = Program::builder();
        let balance = builder.component(ComponentId::new("balance"), Balance);
        let program = builder.build().unwrap();
        let mut runtime = ControlledRuntime::builder(program).build().unwrap();
        for input in inputs {
            runtime.send(&balance, *input).unwrap();
        }
        runtime.run_until_idle().unwrap();
        (*runtime.state(&balance).unwrap(), runtime.trace().to_vec())
    }

    let with_transitions = run(&[1, -1]);
    let without_transitions = run(&[]);
    assert_eq!(with_transitions.0, without_transitions.0);
    assert_ne!(with_transitions.1, without_transitions.1);
}

#[test]
fn v7_controlled_inputs_follow_harness_order() {
    let (program, alpha, _) = timer_program(false);
    let mut runtime = ControlledRuntime::builder(program).build().unwrap();
    runtime
        .send(&alpha, TimerMessage::Mark("harness-first"))
        .unwrap();
    runtime
        .send(&alpha, TimerMessage::Mark("harness-second"))
        .unwrap();
    runtime.run_until_idle().unwrap();

    assert_eq!(
        runtime.state(&alpha).unwrap().0,
        vec!["harness-first", "harness-second"]
    );
}

struct Forwarder {
    target: ComponentRef<TimerComponent>,
}

impl Component for Forwarder {
    type Model = ();
    type Message = ();

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(())
    }

    fn update(&self, _model: &mut Self::Model, _message: Self::Message) -> Command<Self::Message> {
        Command::send(self.target.clone(), TimerMessage::Mark("direct-send"))
    }
}

#[test]
fn v2_cross_component_interaction_reenters_as_message() {
    let mut builder = Program::builder();
    let target = builder.component(ComponentId::new("target"), TimerComponent);
    let forwarder = builder.component(
        ComponentId::new("forwarder"),
        Forwarder {
            target: target.clone(),
        },
    );
    let program = builder.build().unwrap();
    let mut runtime = ControlledRuntime::builder(program).build().unwrap();
    runtime.send(&forwarder, ()).unwrap();
    runtime.run_until_idle().unwrap();

    assert_eq!(runtime.state(&target).unwrap().0, vec!["direct-send"]);
    let send = runtime
        .trace()
        .iter()
        .find(|record| {
            matches!(
                record.event,
                TraceEvent::CommandEmitted {
                    kind: TraceCommandKind::Send,
                    ..
                }
            )
        })
        .expect("direct send command trace");
    assert!(runtime.trace().iter().any(|record| {
        record.cause == Some(send.id)
            && matches!(
                &record.event,
                TraceEvent::Transition { component, .. } if component == target.id()
            )
    }));
}

#[test]
fn v10_trace_records_logical_time_and_exactly_one_immediate_parent_for_non_roots() {
    let (trace, _, _) = run_timers(false, false);
    for (index, record) in trace.iter().enumerate() {
        assert_eq!(record.id.get() as usize, index);
        let root = matches!(
            record.event,
            TraceEvent::Initialization { .. }
                | TraceEvent::ControlledMessageInput { .. }
                | TraceEvent::ControlledSourceInput { .. }
                | TraceEvent::EffectOutcome { .. }
        );
        if root {
            assert_eq!(record.cause, None);
        } else {
            let cause = record
                .cause
                .expect("every non-root has one immediate cause");
            assert!(cause.get() < record.id.get());
            assert!(trace.iter().any(|candidate| candidate.id == cause));
        }
    }
    assert!(
        trace
            .iter()
            .any(|record| record.at.as_duration() == Duration::from_millis(5))
    );
}

#[derive(Debug)]
enum DueMessage {
    Tick,
}

struct DueComponent;

impl Component for DueComponent {
    type Model = usize;
    type Message = DueMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(0).with_command(Command::after(Duration::ZERO, DueMessage::Tick))
    }

    fn update(&self, model: &mut Self::Model, _message: Self::Message) -> Command<Self::Message> {
        *model += 1;
        Command::none()
    }
}

#[test]
fn v9_pending_now_counts_ready_messages_and_due_timers() {
    let mut builder = Program::builder();
    let component = builder.component(ComponentId::new("due"), DueComponent);
    let program = builder.build().unwrap();
    let mut runtime = ControlledRuntime::builder(program).build().unwrap();

    assert_eq!(
        runtime.pending_work().pending_now,
        1,
        "the zero-delay timer is due"
    );
    runtime.send(&component, DueMessage::Tick).unwrap();
    assert_eq!(runtime.pending_work().pending_now, 2);
    let report = runtime.run_until_idle().unwrap();
    assert_eq!(report.transitions, 2);
    assert_eq!(report.pending_now, 0);
    assert_eq!(*runtime.state(&component).unwrap(), 2);
}

#[derive(Debug)]
enum OwnershipMessage {
    Kick,
    Effect(EffectOutcome<u64, Infallible>),
    Request(RequestOutcome<u64>),
    Input(SourceEvent<u64, Infallible>),
    Timer,
}

struct OwnershipComponent {
    port: Port<EchoProtocol>,
    input: StreamDescriptor<u64>,
}

impl Component for OwnershipComponent {
    type Model = ();
    type Message = OwnershipMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(()).with_command(Command::after(
            Duration::from_secs(60),
            OwnershipMessage::Timer,
        ))
    }

    fn update(&self, _model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            OwnershipMessage::Kick => Command::batch([
                Command::effect_with(ProbeEffect(1), OwnershipMessage::Effect),
                Command::request_with(self.port.clone(), Echo(1), OwnershipMessage::Request),
            ]),
            OwnershipMessage::Effect(outcome) => {
                let _ = outcome;
                Command::none()
            }
            OwnershipMessage::Request(outcome) => {
                let _ = outcome;
                Command::none()
            }
            OwnershipMessage::Input(event) => {
                let _ = event;
                Command::none()
            }
            OwnershipMessage::Timer => Command::none(),
        }
    }

    fn subscriptions(&self, _model: &Self::Model) -> Subscriptions<Self::Message> {
        Subscriptions::one(Subscription::source_with(
            SubscriptionId::new("input"),
            self.input.clone(),
            OwnershipMessage::Input,
        ))
    }
}

fn ownership_runtime() -> (ControlledRuntime, ComponentRef<OwnershipComponent>) {
    let mut builder = Program::builder();
    let port = builder.port::<EchoProtocol>(PortId::new("echo"));
    let provider = builder.component(ComponentId::new("provider"), EchoProvider { reply: false });
    builder.bind_port(&port, &provider);
    let input = StreamDescriptor::named("ownership/input");
    let component = builder.component(
        ComponentId::new("owner"),
        OwnershipComponent {
            port,
            input: input.clone(),
        },
    );
    let program = builder.build().unwrap();
    let runtime = ControlledRuntime::builder(program)
        .control_stream(input)
        .control_effect::<ProbeEffect>()
        .build()
        .unwrap();
    (runtime, component)
}

#[test]
fn v9_pending_later_counts_effects_future_timers_sources_and_requests() {
    let (mut runtime, component) = ownership_runtime();
    assert_eq!(
        runtime.pending_work(),
        PendingWork {
            pending_now: 0,
            pending_later: 2,
        }
    );

    runtime.send(&component, OwnershipMessage::Kick).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime.pending_work(),
        PendingWork {
            pending_now: 0,
            pending_later: 4,
        }
    );
}

#[test]
fn v9_controlled_cancel_leaves_zero_work() {
    let (mut runtime, component) = ownership_runtime();
    runtime.send(&component, OwnershipMessage::Kick).unwrap();
    runtime.run_until_idle().unwrap();
    let report = runtime.cancel().unwrap();

    assert_eq!(report.cancelled, 4);
    assert_eq!(report.remaining, 0);
    assert_eq!(report.pending_now, 0);
    assert_eq!(report.pending_later, 0);
}

struct CancellationProbe {
    mapper_calls: Arc<AtomicUsize>,
}

impl Component for CancellationProbe {
    type Model = ();
    type Message = ();

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(())
    }

    fn update(&self, _model: &mut Self::Model, _message: Self::Message) -> Command<Self::Message> {
        Command::none()
    }

    fn subscriptions(&self, _model: &Self::Model) -> Subscriptions<Self::Message> {
        let mapper_calls = self.mapper_calls.clone();
        Subscriptions::one(Subscription::source_with(
            SubscriptionId::new("cancellation-probe"),
            StreamDescriptor::<u64>::named("cancellation-probe"),
            move |_event| {
                mapper_calls.fetch_add(1, Ordering::SeqCst);
            },
        ))
    }
}

#[test]
fn v9_source_cancellation_emits_no_unpromised_event() {
    let mapper_calls = Arc::new(AtomicUsize::new(0));
    let mut builder = Program::builder();
    let _component = builder.component(
        ComponentId::new("cancellation-probe"),
        CancellationProbe {
            mapper_calls: mapper_calls.clone(),
        },
    );
    let program = builder.build().unwrap();
    let runtime = ControlledRuntime::builder(program)
        .control_source::<StreamDescriptor<u64>>()
        .build()
        .unwrap();

    assert_eq!(runtime.pending_work().pending_later, 1);
    let report = runtime.cancel().unwrap();
    assert_eq!(report.cancelled, 1);
    assert_eq!(mapper_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn v10_reading_controlled_trace_after_drive_has_no_feedback_path() {
    let (program, component) = mapping_program();
    let mut runtime = ControlledRuntime::builder(program)
        .control_source::<StreamDescriptor<u64>>()
        .build()
        .unwrap();
    let before = runtime.trace().to_vec();
    assert_eq!(before, runtime.trace());

    runtime
        .emit_source::<MappingComponent, StreamDescriptor<u64>>(
            &component,
            &SubscriptionId::new("input"),
            SourceEvent::Item(3),
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(runtime.state(&component).unwrap().observed, vec![(1, 3)]);
}

// Successful and unanswered typed Requests --------------------------------

struct EchoProtocol;

#[derive(Debug)]
enum EchoProtocolMessage {
    Echo(RequestInvocation<EchoProtocol, Echo>),
}

impl Protocol for EchoProtocol {
    type Message = EchoProtocolMessage;
}

#[derive(Debug)]
struct Echo(u64);

impl Request<EchoProtocol> for Echo {
    type Reply = u64;

    fn into_message(self, reply_to: ReplyTo<Self::Reply>) -> EchoProtocolMessage {
        EchoProtocolMessage::Echo(RequestInvocation::new(self, reply_to))
    }
}

#[derive(Debug)]
enum EchoProviderMessage {
    Protocol(EchoProtocolMessage),
}

impl From<EchoProtocolMessage> for EchoProviderMessage {
    fn from(value: EchoProtocolMessage) -> Self {
        Self::Protocol(value)
    }
}

struct EchoProvider {
    reply: bool,
}

impl Component for EchoProvider {
    type Model = ();
    type Message = EchoProviderMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(())
    }

    fn update(&self, _model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        let EchoProviderMessage::Protocol(EchoProtocolMessage::Echo(invocation)) = message;
        if self.reply {
            Command::reply(invocation.reply_to, invocation.request.0)
        } else {
            let _unanswered = invocation.reply_to;
            Command::none()
        }
    }
}

#[derive(Debug)]
enum EchoRequesterMessage {
    Start,
    Replied {
        slot: usize,
        outcome: RequestOutcome<u64>,
    },
}

#[derive(Default, Debug, PartialEq, Eq)]
struct EchoRequesterModel(Vec<(usize, u64)>);

struct EchoRequester {
    port: Port<EchoProtocol>,
    requests: usize,
}

impl Component for EchoRequester {
    type Model = EchoRequesterModel;
    type Message = EchoRequesterMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(EchoRequesterModel::default())
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            EchoRequesterMessage::Start => Command::batch((0..self.requests).map(|slot| {
                Command::request_with(self.port.clone(), Echo(7), move |outcome| {
                    EchoRequesterMessage::Replied { slot, outcome }
                })
            })),
            EchoRequesterMessage::Replied {
                slot,
                outcome: RequestOutcome::Replied(value),
            } => {
                model.0.push((slot, value));
                Command::none()
            }
            EchoRequesterMessage::Replied { .. } => Command::none(),
        }
    }
}

fn echo_program(reply: bool, requests: usize) -> (Program, ComponentRef<EchoRequester>) {
    let mut builder = Program::builder();
    let port = builder.port::<EchoProtocol>(PortId::new("echo"));
    let provider = builder.component(ComponentId::new("provider"), EchoProvider { reply });
    builder.bind_port(&port, &provider);
    let requester = builder.component(
        ComponentId::new("requester"),
        EchoRequester { port, requests },
    );
    (builder.build().unwrap(), requester)
}

#[test]
fn phase5_successful_request_reply_is_correlated_at_most_once() {
    let (program, requester) = echo_program(true, 2);
    let mut runtime = ControlledRuntime::builder(program).build().unwrap();
    runtime
        .send(&requester, EchoRequesterMessage::Start)
        .unwrap();
    runtime.run_until_idle().unwrap();

    assert_eq!(runtime.state(&requester).unwrap().0, vec![(0, 7), (1, 7)]);
    assert_eq!(runtime.pending_work(), PendingWork::default());
    assert_eq!(
        runtime
            .trace()
            .iter()
            .filter(|record| matches!(record.event, TraceEvent::RequestOutcome { .. }))
            .count(),
        2
    );
}

#[test]
fn v8_request_to_reply_causal_edges_are_preserved() {
    let (program, requester) = echo_program(true, 1);
    let mut runtime = ControlledRuntime::builder(program).build().unwrap();
    runtime
        .send(&requester, EchoRequesterMessage::Start)
        .unwrap();
    runtime.run_until_idle().unwrap();
    let trace = runtime.trace();

    let outcome = trace
        .iter()
        .find(|record| matches!(record.event, TraceEvent::RequestOutcome { .. }))
        .expect("successful request outcome");
    let reply = &trace[outcome.cause.unwrap().get() as usize];
    assert!(matches!(
        reply.event,
        TraceEvent::CommandEmitted {
            kind: TraceCommandKind::Reply,
            ..
        }
    ));
    let provider_transition = &trace[reply.cause.unwrap().get() as usize];
    assert!(matches!(
        &provider_transition.event,
        TraceEvent::Transition { component, .. }
            if component == &ComponentId::new("provider")
    ));
    let request = &trace[provider_transition.cause.unwrap().get() as usize];
    assert!(matches!(
        request.event,
        TraceEvent::CommandEmitted {
            kind: TraceCommandKind::Request,
            ..
        }
    ));
    assert!(trace.iter().any(|record| {
        record.cause == Some(outcome.id)
            && matches!(
                &record.event,
                TraceEvent::Transition { component, .. } if component == requester.id()
            )
    }));
}

#[test]
fn phase5_unanswered_request_remains_pending_until_cancellation() {
    let (program, requester) = echo_program(false, 1);
    let mut runtime = ControlledRuntime::builder(program).build().unwrap();
    runtime
        .send(&requester, EchoRequesterMessage::Start)
        .unwrap();
    let report = runtime.run_until_idle().unwrap();

    assert_eq!(runtime.state(&requester).unwrap().0, Vec::new());
    assert_eq!(report.pending_now, 0);
    assert_eq!(report.pending_later, 1);
    let report = runtime.cancel().unwrap();
    assert!(report.is_clean());
    assert_eq!(report.pending_now, 0);
    assert_eq!(report.pending_later, 0);
}
