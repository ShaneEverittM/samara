use std::any::Any;

use tokio::sync::Mutex;

use crate::{Command, Component, ComponentId, Init};

/// Runtime-owned state and transition mechanism for one Component.
///
/// Exclusive mutable borrowing makes overlapping transitions structurally
/// impossible for an owned kernel. Execution profiles remain free to keep that
/// ownership in one scheduler or place the kernel behind a local serialization
/// boundary when concurrent entry is required.
pub(crate) struct ComponentKernel<C: Component> {
    id: ComponentId,
    component: C,
    model: C::Model,
    initial_command: Option<Command<C::Message>>,
}

impl<C: Component> ComponentKernel<C> {
    pub(crate) fn new(id: ComponentId, component: C) -> Self {
        let Init { model, command } = component.init();

        Self {
            id,
            component,
            model,
            initial_command: Some(command),
        }
    }

    pub(crate) fn id(&self) -> &ComponentId {
        &self.id
    }

    pub(crate) fn transition(&mut self, message: C::Message) -> Command<C::Message> {
        self.component.update(&mut self.model, message)
    }

    pub(crate) fn model(&self) -> &C::Model {
        &self.model
    }

    pub(crate) fn take_initial_command(&mut self) -> Option<Command<C::Message>> {
        self.initial_command.take()
    }
}

/// Optional local serialization boundary for execution profiles that accept
/// concurrent submissions to one Component.
///
/// This wrapper is private runtime machinery rather than Program storage. A
/// synchronously driven controlled runtime can own `ComponentKernel<C>`
/// directly, while a concurrent live topology can choose this or another
/// equivalent mechanism without changing Component semantics.
pub(crate) struct SerializedComponent<C: Component> {
    kernel: Mutex<ComponentKernel<C>>,
}

impl<C: Component> SerializedComponent<C> {
    pub(crate) fn new(kernel: ComponentKernel<C>) -> Self {
        Self {
            kernel: Mutex::new(kernel),
        }
    }

    pub(crate) async fn transition(&self, message: C::Message) -> Command<C::Message> {
        self.kernel.lock().await.transition(message)
    }

    pub(crate) async fn inspect_model<Output>(
        &self,
        inspect: impl FnOnce(&C::Model) -> Output,
    ) -> Output {
        let kernel = self.kernel.lock().await;
        inspect(kernel.model())
    }

    #[cfg(test)]
    async fn transition_observed(
        &self,
        message: C::Message,
        entered: tokio::sync::oneshot::Sender<()>,
        release: tokio::sync::oneshot::Receiver<()>,
    ) -> Command<C::Message> {
        let mut kernel = self.kernel.lock().await;
        let _ = entered.send(());
        let _ = release.await;
        kernel.transition(message)
    }
}

/// Type-erased ownership used by the topology-neutral Program blueprint.
///
/// Message delivery remains typed in `ComponentKernel<C>`; erasure here only
/// permits a Program to retain heterogeneous Component configurations and
/// Models until a later runtime phase chooses how to route typed Messages.
pub(crate) trait ErasedComponentKernel: Send {
    fn id(&self) -> &ComponentId;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<C: Component> ErasedComponentKernel for ComponentKernel<C> {
    fn id(&self) -> &ComponentId {
        self.id()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        future::Future,
        marker::PhantomData,
        pin::Pin,
        task::{Context, Poll, Waker},
        time::Duration,
    };

    use super::{ComponentKernel, SerializedComponent};
    use crate::{Command, Component, ComponentId, Init};
    use tokio::sync::oneshot;

    struct ProbeComponent {
        not_sync: PhantomData<Cell<()>>,
    }

    impl Component for ProbeComponent {
        type Model = Vec<usize>;
        type Message = usize;

        fn init(&self) -> Init<Self::Model, Self::Message> {
            Init::new(Vec::new())
        }

        fn update(
            &self,
            model: &mut Self::Model,
            message: Self::Message,
        ) -> Command<Self::Message> {
            model.push(message);
            Command::none()
        }
    }

    fn assert_pending<F: Future>(future: Pin<&mut F>) {
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(future.poll(&mut context), Poll::Pending));
    }

    fn assert_ready<F: Future>(future: Pin<&mut F>) -> F::Output {
        let mut context = Context::from_waker(Waker::noop());
        match future.poll(&mut context) {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("future remained pending"),
        }
    }

    async fn assert_local_serialization(id: &str) {
        let component = SerializedComponent::new(ComponentKernel::new(
            ComponentId::new(id),
            ProbeComponent {
                not_sync: PhantomData,
            },
        ));

        let (left_entered, mut observe_left_entered) = oneshot::channel();
        let (release_left, left_release) = oneshot::channel();
        let (right_entered, mut observe_right_entered) = oneshot::channel();
        let (release_right, right_release) = oneshot::channel();
        let mut left = Box::pin(component.transition_observed(1, left_entered, left_release));
        let mut right = Box::pin(component.transition_observed(2, right_entered, right_release));

        // The first transition enters its semantic critical section and pauses
        // in test-only instrumentation before update. Polling the second
        // submission cannot enter that section until the first is released.
        // This is deterministic and independent of worker count, wall time,
        // queue shape, or Component-side blocking.
        assert_pending(left.as_mut());
        assert_eq!(observe_left_entered.try_recv(), Ok(()));
        assert_pending(right.as_mut());
        assert!(matches!(
            observe_right_entered.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));

        release_left.send(()).expect("left transition was retained");
        assert!(assert_ready(left.as_mut()).is_none());
        assert_pending(right.as_mut());
        assert_eq!(observe_right_entered.try_recv(), Ok(()));
        release_right
            .send(())
            .expect("right transition was retained");
        assert!(assert_ready(right.as_mut()).is_none());

        assert_eq!(component.inspect_model(Vec::len).await, 2);
    }

    #[tokio::test]
    async fn v2_same_component_transitions_never_overlap() {
        assert_local_serialization("probe").await;
    }

    #[tokio::test]
    async fn v8_component_transitions_do_not_overlap() {
        assert_local_serialization("causal/probe").await;
    }

    #[test]
    fn startup_command_is_retained_and_claimed_once() {
        struct Startup;

        impl Component for Startup {
            type Model = ();
            type Message = ();

            fn init(&self) -> Init<Self::Model, Self::Message> {
                Init::new(()).with_command(Command::after(Duration::ZERO, ()))
            }

            fn update(
                &self,
                _model: &mut Self::Model,
                _message: Self::Message,
            ) -> Command<Self::Message> {
                Command::none()
            }
        }

        let mut kernel = ComponentKernel::new(ComponentId::new("startup"), Startup);

        assert!(kernel.take_initial_command().is_some());
        assert!(kernel.take_initial_command().is_none());
    }
}
