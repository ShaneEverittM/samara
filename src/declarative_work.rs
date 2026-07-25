//! Profile-independent declarative-work machinery.
//!
//! Phase 4 introduced this descriptor comparison independently of a runtime.
//! Phase 5 consumes its inert changes to maintain controlled Sources: a retained
//! change installs the newest mapper while preserving SourcePlan state, and a
//! replacement creates a fresh private generation.
//!
//! Reconciliation commits descriptor bookkeeping while returning an unordered
//! set of lifecycle changes. Execution profiles accept those changes as one
//! reconciliation step rather than treating Vec traversal as application Source
//! ordering.

use std::{collections::HashSet, marker::PhantomData};

use crate::{ComponentId, ErasedSourceDescriptor, Subscription, SubscriptionId, Subscriptions};

/// Descriptor-only lifecycle comparison for one Component's subscriptions.
pub(crate) struct SubscriptionReconciler<Message> {
    owner: ComponentId,
    active: Vec<ActiveDescriptor>,
    marker: PhantomData<fn() -> Message>,
}

struct ActiveDescriptor {
    id: SubscriptionId,
    descriptor: Box<dyn ErasedSourceDescriptor>,
}

/// One Source-lifecycle decision produced without realizing that Source.
///
/// The surrounding Vec is a collection, not a promised application order.
pub(crate) enum SubscriptionChange<Message> {
    /// A new Component-local identity needs a Source.
    Start { subscription: Subscription<Message> },
    /// The identity and descriptor are unchanged, so its Source is retained.
    Retain { subscription: Subscription<Message> },
    /// The identity remains but changed descriptor data requires replacement.
    Replace { subscription: Subscription<Message> },
    /// A formerly active identity is no longer desired.
    Cancel { id: SubscriptionId },
}

/// Invalid desired-subscription set detected before bookkeeping changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DuplicateSubscriptionIdError {
    id: SubscriptionId,
}

impl DuplicateSubscriptionIdError {
    pub(crate) fn id(&self) -> &SubscriptionId {
        &self.id
    }
}

impl<Message> SubscriptionReconciler<Message> {
    pub(crate) fn new(owner: ComponentId) -> Self {
        Self {
            owner,
            active: Vec::new(),
            marker: PhantomData,
        }
    }

    pub(crate) fn owner(&self) -> &ComponentId {
        &self.owner
    }

    pub(crate) fn reconcile(
        &mut self,
        desired: Subscriptions<Message>,
    ) -> Result<Vec<SubscriptionChange<Message>>, DuplicateSubscriptionIdError> {
        let mut desired_ids = HashSet::new();
        for subscription in &desired.0 {
            if !desired_ids.insert(subscription.id().clone()) {
                return Err(DuplicateSubscriptionIdError {
                    id: subscription.id().clone(),
                });
            }
        }

        let mut changes = Vec::new();

        for subscription in desired.0 {
            match self
                .active
                .iter_mut()
                .find(|active| active.id == *subscription.id())
            {
                None => {
                    self.active.push(ActiveDescriptor {
                        id: subscription.id().clone(),
                        descriptor: subscription.descriptor_snapshot(),
                    });
                    changes.push(SubscriptionChange::Start { subscription });
                }
                Some(active) if subscription.has_descriptor(active.descriptor.as_ref()) => {
                    changes.push(SubscriptionChange::Retain { subscription });
                }
                Some(active) => {
                    active.descriptor = subscription.descriptor_snapshot();
                    changes.push(SubscriptionChange::Replace { subscription });
                }
            }
        }

        let mut index = 0;
        while index < self.active.len() {
            if desired_ids.contains(&self.active[index].id) {
                index += 1;
            } else {
                let active = self.active.remove(index);
                changes.push(SubscriptionChange::Cancel { id: active.id });
            }
        }

        Ok(changes)
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use super::{SubscriptionChange, SubscriptionReconciler};
    use crate::component_kernel::ComponentKernel;
    use crate::{
        Command, Component, ComponentId, EffectDescriptor, EffectOutcome, Init, SourceEvent,
        StreamDescriptor, Subscription, SubscriptionId, Subscriptions,
    };

    #[derive(Debug, PartialEq, Eq)]
    struct Write(u64);

    impl EffectDescriptor for Write {
        type Output = u64;
        type Error = Infallible;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct Written {
        request: u64,
        output: u64,
    }

    #[derive(Debug, PartialEq, Eq)]
    enum Message {
        Old(SourceEvent<u64, Infallible>),
        New(SourceEvent<u64, Infallible>),
    }

    fn desired(binding: &'static str) -> Subscriptions<Message> {
        Subscriptions::one(Subscription::source_with(
            SubscriptionId::new("input"),
            StreamDescriptor::<u64>::named(binding),
            Message::Old,
        ))
    }

    struct DesiredComponent;

    enum DesiredMessage {
        Use(&'static str),
        Stop,
        Input(SourceEvent<u64, Infallible>),
    }

    impl Component for DesiredComponent {
        type Model = Option<&'static str>;
        type Message = DesiredMessage;

        fn init(&self) -> Init<Self::Model, Self::Message> {
            Init::new(Some("one"))
        }

        fn update(
            &self,
            model: &mut Self::Model,
            message: Self::Message,
        ) -> Command<Self::Message> {
            match message {
                DesiredMessage::Use(binding) => *model = Some(binding),
                DesiredMessage::Stop => *model = None,
                DesiredMessage::Input(_) => {}
            }
            Command::none()
        }

        fn subscriptions(&self, model: &Self::Model) -> Subscriptions<Self::Message> {
            match model {
                Some(binding) => Subscriptions::one(Subscription::source_with(
                    SubscriptionId::new("input"),
                    StreamDescriptor::<u64>::named(*binding),
                    DesiredMessage::Input,
                )),
                None => Subscriptions::none(),
            }
        }
    }

    #[test]
    fn v3_batched_effect_occurrences_keep_distinct_one_shot_mappers() {
        let command = Command::batch([
            Command::effect_with(Write(7), |outcome| {
                let EffectOutcome::Succeeded(output) = outcome else {
                    unreachable!("the fixture supplies success")
                };
                Written { request: 1, output }
            }),
            Command::effect_with(Write(7), |outcome| {
                let EffectOutcome::Succeeded(output) = outcome else {
                    unreachable!("the fixture supplies success")
                };
                Written { request: 2, output }
            }),
        ]);

        let mut declarations = command.into_declarations().into_iter();
        let first = declarations.next().expect("first occurrence");
        let second = declarations.next().expect("second occurrence");
        assert!(declarations.next().is_none());

        let first = first
            .into_effect::<Write>()
            .unwrap_or_else(|_| panic!("first descriptor should match"));
        let (descriptor, mapper) = first.into_parts();
        assert_eq!(descriptor, Write(7));
        assert_eq!(
            mapper(EffectOutcome::Succeeded(70)),
            Written {
                request: 1,
                output: 70
            }
        );
        assert_eq!(
            second
                .map_effect_outcome::<Write>(EffectOutcome::Succeeded(71))
                .unwrap_or_else(|_| panic!("second descriptor should match")),
            Written {
                request: 2,
                output: 71
            }
        );
    }

    #[test]
    fn v4_component_kernel_reconciles_the_committed_model_projection() {
        let mut kernel = ComponentKernel::new(ComponentId::new("desired"), DesiredComponent);

        assert!(matches!(
            kernel
                .reconcile_subscriptions()
                .expect("valid initial desires")
                .as_slice(),
            [SubscriptionChange::Start { .. }]
        ));
        let _ = kernel.transition(DesiredMessage::Use("one"));
        assert!(matches!(
            kernel
                .reconcile_subscriptions()
                .expect("valid retained desires")
                .as_slice(),
            [SubscriptionChange::Retain { .. }]
        ));
        let _ = kernel.transition(DesiredMessage::Use("two"));
        assert!(matches!(
            kernel
                .reconcile_subscriptions()
                .expect("valid replacement desires")
                .as_slice(),
            [SubscriptionChange::Replace { .. }]
        ));
        let _ = kernel.transition(DesiredMessage::Stop);
        assert!(matches!(
            kernel
                .reconcile_subscriptions()
                .expect("valid removed desires")
                .as_slice(),
            [SubscriptionChange::Cancel { .. }]
        ));
    }

    #[test]
    fn v4_new_equal_changed_and_removed_descriptors_reconcile_lifecycle() {
        let mut reconciler = SubscriptionReconciler::new(ComponentId::new("alpha"));

        assert!(matches!(
            reconciler
                .reconcile(desired("one"))
                .expect("valid desires")
                .as_slice(),
            [SubscriptionChange::Start { .. }]
        ));
        assert!(matches!(
            reconciler
                .reconcile(desired("one"))
                .expect("valid desires")
                .as_slice(),
            [SubscriptionChange::Retain { .. }]
        ));
        assert!(matches!(
            reconciler
                .reconcile(desired("two"))
                .expect("valid desires")
                .as_slice(),
            [SubscriptionChange::Replace { .. }]
        ));
        assert!(matches!(
            reconciler
                .reconcile(Subscriptions::none())
                .expect("valid desires")
                .as_slice(),
            [SubscriptionChange::Cancel { .. }]
        ));
    }

    #[test]
    fn v4_subscription_identity_is_local_to_one_component() {
        let mut alpha = SubscriptionReconciler::new(ComponentId::new("alpha"));
        let mut beta = SubscriptionReconciler::new(ComponentId::new("beta"));

        assert_eq!(alpha.owner(), &ComponentId::new("alpha"));
        assert_eq!(beta.owner(), &ComponentId::new("beta"));

        assert!(matches!(
            alpha
                .reconcile(desired("shared"))
                .expect("valid desires")
                .as_slice(),
            [SubscriptionChange::Start { .. }]
        ));
        assert!(matches!(
            beta.reconcile(desired("shared"))
                .expect("valid desires")
                .as_slice(),
            [SubscriptionChange::Start { .. }]
        ));
    }

    #[test]
    fn duplicate_desired_identity_is_rejected_before_bookkeeping_changes() {
        let mut reconciler = SubscriptionReconciler::new(ComponentId::new("alpha"));
        let duplicate = vec![
            Subscription::source_with(
                SubscriptionId::new("input"),
                StreamDescriptor::<u64>::named("one"),
                Message::Old,
            ),
            Subscription::source_with(
                SubscriptionId::new("input"),
                StreamDescriptor::<u64>::named("two"),
                Message::New,
            ),
        ]
        .into();

        let error = match reconciler.reconcile(duplicate) {
            Ok(_) => panic!("one Component cannot desire one identity twice"),
            Err(error) => error,
        };
        assert_eq!(error.id, SubscriptionId::new("input"));
        assert!(matches!(
            reconciler
                .reconcile(desired("one"))
                .expect("failed reconciliation was transactional")
                .as_slice(),
            [SubscriptionChange::Start { .. }]
        ));
    }

    #[test]
    fn v4_retention_preserves_new_mapper_as_unselected_candidate_data() {
        let mut reconciler = SubscriptionReconciler::new(ComponentId::new("alpha"));
        let _ = reconciler.reconcile(desired("one")).expect("valid desires");
        let changes = reconciler
            .reconcile(Subscriptions::one(Subscription::source_with(
                SubscriptionId::new("input"),
                StreamDescriptor::<u64>::named("one"),
                Message::New,
            )))
            .expect("valid desires");

        let [SubscriptionChange::Retain { subscription }] = changes.as_slice() else {
            panic!("an equal descriptor should retain its Source");
        };
        assert_eq!(
            subscription
                .map_source_event::<StreamDescriptor<u64>>(SourceEvent::Item(9))
                .expect("the desired mapper is still available"),
            Message::New(SourceEvent::Item(9))
        );
    }
}
