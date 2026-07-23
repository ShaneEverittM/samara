//! Executable acceptance evidence for the Phase 3 Component kernel.
//!
//! These tests stay below Command interpretation and runtime-profile policy.
//! They exercise the frozen Component signature, deterministic direct
//! transitions, explicit logical identity, and ownership transfer at program
//! assembly. Private kernel tests cover concurrent serialization.

use std::{
    any::TypeId,
    cell::Cell,
    marker::PhantomData,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use samara::prelude::*;

struct KernelProtocol;

enum KernelProtocolMessage {
    Add(u64),
}

enum ProviderMessage {
    Protocol(KernelProtocolMessage),
}

impl Protocol for KernelProtocol {
    type Message = KernelProtocolMessage;
}

impl From<KernelProtocolMessage> for ProviderMessage {
    fn from(message: KernelProtocolMessage) -> Self {
        Self::Protocol(message)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct KernelModel {
    total: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum KernelMessage {
    Add(u64),
}

struct KernelComponent {
    scale: u64,
    init_calls: Arc<AtomicUsize>,
    configuration_drops: Arc<AtomicUsize>,
    model_drops: Arc<AtomicUsize>,
    not_sync: PhantomData<Cell<()>>,
}

impl Drop for KernelComponent {
    fn drop(&mut self) {
        self.configuration_drops.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Debug)]
struct OwnedModel {
    state: KernelModel,
    drops: Arc<AtomicUsize>,
}

impl Drop for OwnedModel {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

impl PartialEq for OwnedModel {
    fn eq(&self, other: &Self) -> bool {
        self.state == other.state
    }
}

impl Eq for OwnedModel {}

impl KernelComponent {
    fn fixture() -> (Self, Arc<AtomicUsize>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let init_calls = Arc::new(AtomicUsize::new(0));
        let configuration_drops = Arc::new(AtomicUsize::new(0));
        let model_drops = Arc::new(AtomicUsize::new(0));

        (
            Self {
                scale: 3,
                init_calls: init_calls.clone(),
                configuration_drops: configuration_drops.clone(),
                model_drops: model_drops.clone(),
                not_sync: PhantomData,
            },
            init_calls,
            configuration_drops,
            model_drops,
        )
    }
}

impl Component for KernelComponent {
    type Model = OwnedModel;
    type Message = KernelMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        self.init_calls.fetch_add(1, Ordering::SeqCst);
        Init::new(OwnedModel {
            state: KernelModel::default(),
            drops: self.model_drops.clone(),
        })
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            KernelMessage::Add(amount) => model.state.total += amount * self.scale,
        }
        Command::none()
    }
}

#[test]
fn v1_same_input_produces_equivalent_model_and_command_intent() {
    let (component, _, _, _) = KernelComponent::fixture();
    let mut left = component.init().model;
    let mut right = component.init().model;

    let left_command = component.update(&mut left, KernelMessage::Add(7));
    let right_command = component.update(&mut right, KernelMessage::Add(7));

    assert!(left_command.is_none());
    assert!(right_command.is_none());
    assert_eq!(left, right);
}

#[test]
fn v1_update_has_no_runtime_capability() {
    let update: fn(&KernelComponent, &mut OwnedModel, KernelMessage) -> Command<KernelMessage> =
        KernelComponent::update;

    let _ = update;
}

#[test]
fn v2_protocol_message_remains_distinct_from_provider_message() {
    fn assert_provider_conversion<Message: From<KernelProtocolMessage>>() {}

    let _protocol = KernelProtocol;
    assert_provider_conversion::<ProviderMessage>();
    assert_ne!(
        TypeId::of::<KernelProtocolMessage>(),
        TypeId::of::<ProviderMessage>()
    );

    let ProviderMessage::Protocol(KernelProtocolMessage::Add(amount)) =
        ProviderMessage::from(KernelProtocolMessage::Add(5));
    assert_eq!(amount, 5);
}

#[test]
fn component_identity_is_explicit_and_not_registration_order() {
    let mut left_program = Program::builder();
    let (left_first, _, _, _) = KernelComponent::fixture();
    let (left_second, _, _, _) = KernelComponent::fixture();
    let alpha_left = left_program.component(ComponentId::new("alpha"), left_first);
    let beta_left = left_program.component(ComponentId::new("beta"), left_second);

    let mut right_program = Program::builder();
    let (right_first, _, _, _) = KernelComponent::fixture();
    let (right_second, _, _, _) = KernelComponent::fixture();
    let beta_right = right_program.component(ComponentId::new("beta"), right_first);
    let alpha_right = right_program.component(ComponentId::new("alpha"), right_second);

    assert_eq!(alpha_left.id(), alpha_right.id());
    assert_eq!(beta_left.id(), beta_right.id());
    assert_ne!(alpha_left.id(), beta_left.id());
}

#[test]
fn registration_transfers_configuration_and_model_ownership_to_program() {
    let (component, init_calls, configuration_drops, model_drops) = KernelComponent::fixture();
    let mut builder = Program::builder();

    let component_ref = builder.component(ComponentId::new("owned"), component);

    assert_eq!(component_ref.id(), &ComponentId::new("owned"));
    assert_eq!(configuration_drops.load(Ordering::SeqCst), 0);
    assert_eq!(model_drops.load(Ordering::SeqCst), 0);

    let program = builder.build().expect("the ownership graph is valid");

    // The contract requires one initial Model and startup Command before the
    // Program can execute; it does not make component-registration versus
    // build timing observable.
    assert_eq!(init_calls.load(Ordering::SeqCst), 1);
    assert_eq!(configuration_drops.load(Ordering::SeqCst), 0);
    assert_eq!(model_drops.load(Ordering::SeqCst), 0);

    drop(program);

    assert_eq!(configuration_drops.load(Ordering::SeqCst), 1);
    assert_eq!(model_drops.load(Ordering::SeqCst), 1);
}
