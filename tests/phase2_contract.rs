//! Executable evidence for the Phase 2 Component-kernel contract.
//!
//! These tests exercise only behavior that the compiler-checked façade can
//! honestly provide before runtime implementation. Later V1-V11 scenarios are
//! staged in `docs/testing/v0-acceptance-matrix.md` rather than made green with
//! placeholder runtime behavior.

use std::{cell::Cell, convert::Infallible, marker::PhantomData};

use samara::prelude::*;

const INPUT_BINDING: &str = "phase2/external-input";
const INPUT_SUBSCRIPTION: &str = "input";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ContractModel {
    total: u64,
    input_closed: bool,
    last_save: Option<EffectOutcome<(), SaveError>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ContractMessage {
    Add(u64),
    Input(SourceEvent<u64, Infallible>),
    Saved(EffectOutcome<(), SaveError>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SaveTotal {
    value: u64,
}

impl EffectDescriptor for SaveTotal {
    type Output = ();
    type Error = SaveError;
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SaveError;

/// The marker makes this configuration `!Sync` without adding behavioral
/// mutable state. Compiling its Component implementation proves that the base
/// kernel contract does not require scheduler-driven shared access to a
/// Component configuration.
struct ContractComponent;

struct ContractComponentConfig {
    source: SourceCapability<StreamDescriptor<u64>>,
    save: EffectCapability<SaveTotal>,
    input: StreamDescriptor<u64>,
    not_sync: PhantomData<Cell<()>>,
}

impl ContractComponentConfig {
    fn new(program: &mut ProgramBuilder) -> Self {
        Self {
            source: program.source::<StreamDescriptor<u64>>(),
            save: program.effect::<SaveTotal>(),
            input: StreamDescriptor::named(INPUT_BINDING),
            not_sync: PhantomData,
        }
    }
}

impl Component for ContractComponent {
    type Config = ContractComponentConfig;
    type Model = ContractModel;
    type Message = ContractMessage;

    fn init(_config: &Self::Config) -> Init<Self::Model, Self::Message> {
        Init::new(ContractModel::default())
    }

    fn update(
        config: &Self::Config,
        model: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        match message {
            ContractMessage::Add(amount) => {
                model.total += amount;
                Command::effect_with(
                    &config.save,
                    SaveTotal { value: model.total },
                    ContractMessage::Saved,
                )
            }
            ContractMessage::Input(SourceEvent::Item(amount)) => {
                model.total += amount;
                Command::none()
            }
            ContractMessage::Input(SourceEvent::Ended) => {
                model.input_closed = true;
                Command::none()
            }
            ContractMessage::Input(SourceEvent::Failed(never)) => match never {},
            ContractMessage::Saved(outcome) => {
                model.last_save = Some(outcome);
                Command::none()
            }
        }
    }

    fn subscriptions(config: &Self::Config, model: &Self::Model) -> Subscriptions<Self::Message> {
        if model.input_closed {
            return Subscriptions::none();
        }

        Subscriptions::one(Subscription::source_with(
            &config.source,
            SubscriptionId::new(INPUT_SUBSCRIPTION),
            config.input.clone(),
            ContractMessage::Input,
        ))
    }
}

#[test]
fn v1_same_input_produces_equivalent_model_and_command_intent() {
    let mut program = Program::builder();
    let component = ContractComponentConfig::new(&mut program);
    let mut left = ContractComponent::init(&component).model;
    let mut right = ContractComponent::init(&component).model;

    let left_command = ContractComponent::update(&component, &mut left, ContractMessage::Add(7));
    let right_command = ContractComponent::update(&component, &mut right, ContractMessage::Add(7));

    assert_eq!(left, right);
    assert_eq!(
        left_command.effect_intent::<SaveTotal>(),
        right_command.effect_intent::<SaveTotal>()
    );
}

#[test]
fn v3_effect_command_exposes_typed_descriptor() {
    let mut program = Program::builder();
    let component = ContractComponentConfig::new(&mut program);
    let mut model = ContractComponent::init(&component).model;

    let command = ContractComponent::update(&component, &mut model, ContractMessage::Add(11));

    assert_eq!(
        command.effect_intent::<SaveTotal>(),
        Some(&SaveTotal { value: 11 })
    );
    assert_eq!(
        command.effect_type_name(),
        Some(std::any::type_name::<SaveTotal>())
    );
}

#[test]
fn v4_subscription_separates_component_identity_from_source_descriptor() {
    let mut program = Program::builder();
    let component = ContractComponentConfig::new(&mut program);
    let model = ContractComponent::init(&component).model;

    let subscriptions = ContractComponent::subscriptions(&component, &model);
    let subscription = subscriptions.iter().next().expect("one input desire");

    assert_eq!(subscription.id(), &SubscriptionId::new(INPUT_SUBSCRIPTION));
    assert_eq!(
        subscription.source_descriptor::<StreamDescriptor<u64>>(),
        Some(&StreamDescriptor::named(INPUT_BINDING))
    );
}

#[test]
fn component_kernel_accepts_non_sync_configuration() {
    fn assert_component<C: Component>() {}
    assert_component::<ContractComponent>();

    let mut program = Program::builder();
    let contract = ContractComponentConfig::new(&mut program);
    let component = program.component::<ContractComponent>(ComponentId::new("contract"), contract);

    assert_eq!(component.id(), &ComponentId::new("contract"));
}
