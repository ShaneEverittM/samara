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
struct ContractComponent {
    input: StreamDescriptor<u64>,
    not_sync: PhantomData<Cell<()>>,
}

impl ContractComponent {
    fn new() -> Self {
        Self {
            input: StreamDescriptor::named(INPUT_BINDING),
            not_sync: PhantomData,
        }
    }
}

impl Component for ContractComponent {
    type Model = ContractModel;
    type Message = ContractMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(ContractModel::default())
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            ContractMessage::Add(amount) => {
                model.total += amount;
                Command::effect_with(SaveTotal { value: model.total }, ContractMessage::Saved)
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

    fn subscriptions(&self, model: &Self::Model) -> Subscriptions<Self::Message> {
        if model.input_closed {
            return Subscriptions::none();
        }

        Subscriptions::one(Subscription::source_with(
            SubscriptionId::new(INPUT_SUBSCRIPTION),
            self.input.clone(),
            ContractMessage::Input,
        ))
    }
}

#[test]
fn v1_same_input_produces_equivalent_model_and_command_intent() {
    let component = ContractComponent::new();
    let mut left = component.init().model;
    let mut right = component.init().model;

    let left_command = component.update(&mut left, ContractMessage::Add(7));
    let right_command = component.update(&mut right, ContractMessage::Add(7));

    assert_eq!(left, right);
    assert_eq!(
        left_command.effect_intent::<SaveTotal>(),
        right_command.effect_intent::<SaveTotal>()
    );
}

#[test]
fn v3_effect_command_exposes_typed_descriptor() {
    let component = ContractComponent::new();
    let mut model = component.init().model;

    let command = component.update(&mut model, ContractMessage::Add(11));

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
    let component = ContractComponent::new();
    let model = component.init().model;

    let subscriptions = component.subscriptions(&model);
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
    let component = program.component(ComponentId::new("contract"), ContractComponent::new());

    assert_eq!(component.id(), &ComponentId::new("contract"));
}
