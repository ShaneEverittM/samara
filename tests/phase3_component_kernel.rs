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

enum KernelProtocolMessage {
    Add(u64),
}

enum ProviderMessage {
    Protocol(KernelProtocolMessage),
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

struct KernelComponent;

struct KernelComponentConfig {
    scale: u64,
    init_calls: Arc<AtomicUsize>,
    configuration_drops: Arc<AtomicUsize>,
    model_drops: Arc<AtomicUsize>,
    not_sync: PhantomData<Cell<()>>,
}

impl Drop for KernelComponentConfig {
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

impl KernelComponentConfig {
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
    type Requires = ();
    type Config = KernelComponentConfig;
    type Model = OwnedModel;
    type Message = KernelMessage;

    fn init(
        config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
    ) -> Init<Self::Model, Self::Message> {
        config.init_calls.fetch_add(1, Ordering::SeqCst);
        Init::new(OwnedModel {
            state: KernelModel::default(),
            drops: config.model_drops.clone(),
        })
    }

    fn update(
        config: &Self::Config,
        _: &samara::Capabilities<Self::Requires>,
        model: &mut Self::Model,
        message: Self::Message,
    ) -> Command<Self::Message> {
        match message {
            KernelMessage::Add(amount) => model.state.total += amount * config.scale,
        }
        Command::none()
    }
}

#[test]
fn v1_same_input_produces_equivalent_model_and_command_intent() {
    let (component, _, _, _) = KernelComponentConfig::fixture();
    let mut left = KernelComponent::init(&component, &()).model;
    let mut right = KernelComponent::init(&component, &()).model;

    let left_command = KernelComponent::update(&component, &(), &mut left, KernelMessage::Add(7));
    let right_command = KernelComponent::update(&component, &(), &mut right, KernelMessage::Add(7));

    assert!(left_command.is_none());
    assert!(right_command.is_none());
    assert_eq!(left, right);
}

#[test]
fn v1_update_has_no_runtime_capability() {
    let update: fn(
        &KernelComponentConfig,
        &(),
        &mut OwnedModel,
        KernelMessage,
    ) -> Command<KernelMessage> = KernelComponent::update;

    let _ = update;
}

#[test]
fn v2_protocol_message_remains_distinct_from_provider_message() {
    fn assert_provider_conversion<Message: From<KernelProtocolMessage>>() {}

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
fn component_identity_follows_registration_independently_of_names() {
    let (mut left_program, ()) = samara::Program::builder::<()>();
    let (left_first, _, _, _) = KernelComponentConfig::fixture();
    let (left_second, _, _, _) = KernelComponentConfig::fixture();
    let alpha_left = left_program
        .component::<KernelComponent>(left_first)
        .name("alpha")
        .finish()
        .expect("component registration");
    let beta_left = left_program
        .component::<KernelComponent>(left_second)
        .name("beta")
        .finish()
        .expect("component registration");

    let (mut right_program, ()) = samara::Program::builder::<()>();
    let (right_first, _, _, _) = KernelComponentConfig::fixture();
    let (right_second, _, _, _) = KernelComponentConfig::fixture();
    let beta_right = right_program
        .component::<KernelComponent>(right_first)
        .name("beta")
        .finish()
        .expect("component registration");
    let alpha_right = right_program
        .component::<KernelComponent>(right_second)
        .name("alpha")
        .finish()
        .expect("component registration");

    assert_eq!(alpha_left.id(), beta_right.id());
    assert_eq!(beta_left.id(), alpha_right.id());
    assert_ne!(alpha_left.id(), beta_left.id());
}

#[test]
fn registration_transfers_configuration_and_model_ownership_to_program() {
    let (component, init_calls, configuration_drops, model_drops) =
        KernelComponentConfig::fixture();
    let (mut builder, ()) = samara::Program::builder::<()>();

    let component_ref = builder
        .component::<KernelComponent>(component)
        .name("owned")
        .finish()
        .expect("component registration");

    assert_eq!(component_ref.id().name(), "owned");
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
