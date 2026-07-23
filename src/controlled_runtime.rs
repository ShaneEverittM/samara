//! Synchronous deterministic execution for the Phase 5 controlled profile.
//!
//! The implementation deliberately uses one owned scheduler because that is a
//! small conforming mechanism, not because Samara promises a global live loop.
//! Its observable order is the ADR-0003 key `(logical deadline, insertion
//! ticket)` and all queued work retains explicit causation.

use std::{
    any::{Any, TypeId},
    collections::HashSet,
    sync::Arc,
    time::Duration,
};

use crate::declarative_work::SubscriptionChange;
use crate::{
    Command, CommandKind, Component, ComponentId, ComponentRef, EffectDescriptor, EffectOutcome,
    EffectOutcomeKind, ErasedRequestMapper, ErasedSourceDescriptor, ErasedSourceEvent, LogicalTime,
    PendingEffect, PendingWork, PortId, Program, RunReport, RuntimeError, ShutdownReport,
    SourceDescriptor, SourceDescriptorSnapshot, SourceEvent, SourceEventKind, SourcePlan,
    StreamDescriptor, Subscription, SubscriptionAction, SubscriptionId, TraceCommandKind,
    TraceEvent, TraceId, TraceRecord,
};

type ErasedValue = Box<dyn Any + Send>;
type ErasedMessageMapper = Box<dyn FnOnce(ErasedValue) -> ErasedValue + Send + 'static>;

/// Type-erased finite work returned by one typed Component transition.
pub(crate) trait ErasedCommand: Send {
    fn interpret(
        self: Box<Self>,
        runtime: &mut ControlledCore,
        component: &ComponentId,
        cause: TraceId,
    ) -> Result<(), RuntimeError>;
}

struct TypedCommand<Message>(Command<Message>);

pub(crate) fn erase_command<Message>(command: Command<Message>) -> Box<dyn ErasedCommand>
where
    Message: Send + 'static,
{
    Box::new(TypedCommand(command))
}

impl<Message> ErasedCommand for TypedCommand<Message>
where
    Message: Send + 'static,
{
    fn interpret(
        self: Box<Self>,
        runtime: &mut ControlledCore,
        component: &ComponentId,
        cause: TraceId,
    ) -> Result<(), RuntimeError> {
        runtime.interpret_command(self.0, component, cause)
    }
}

pub(crate) trait ErasedRuntimeSubscription: Send {
    fn id(&self) -> &SubscriptionId;
    fn descriptor(&self) -> &dyn Any;
    fn descriptor_type_name(&self) -> &'static str;
    fn message_type_id(&self) -> TypeId;
    fn message_type_name(&self) -> &'static str;
    fn source_plan(&self) -> SourcePlan;
    fn source_event_type_id(&self) -> TypeId;
    fn map_event(&self, event: ErasedSourceEvent) -> ErasedValue;
}

struct TypedRuntimeSubscription<Message>(Subscription<Message>);

impl<Message> ErasedRuntimeSubscription for TypedRuntimeSubscription<Message>
where
    Message: Send + 'static,
{
    fn id(&self) -> &SubscriptionId {
        self.0.id()
    }

    fn descriptor(&self) -> &dyn Any {
        self.0.descriptor_any()
    }

    fn descriptor_type_name(&self) -> &'static str {
        self.0.descriptor_type_name()
    }

    fn message_type_id(&self) -> TypeId {
        TypeId::of::<Message>()
    }

    fn message_type_name(&self) -> &'static str {
        std::any::type_name::<Message>()
    }

    fn source_plan(&self) -> SourcePlan {
        self.0.source_plan()
    }

    fn source_event_type_id(&self) -> TypeId {
        self.0.source_event_type_id()
    }

    fn map_event(&self, event: ErasedSourceEvent) -> ErasedValue {
        Box::new(self.0.map_erased_source_event(event))
    }
}

fn erase_subscription<Message>(
    subscription: Subscription<Message>,
) -> Box<dyn ErasedRuntimeSubscription>
where
    Message: Send + 'static,
{
    Box::new(TypedRuntimeSubscription(subscription))
}

/// Type-erased Source lifecycle decision returned by a Component kernel.
pub(crate) enum ErasedSubscriptionChange {
    Start(Box<dyn ErasedRuntimeSubscription>),
    Retain(Box<dyn ErasedRuntimeSubscription>),
    Replace(Box<dyn ErasedRuntimeSubscription>),
    Cancel(SubscriptionId),
}

impl ErasedSubscriptionChange {
    fn id(&self) -> &SubscriptionId {
        match self {
            Self::Start(subscription)
            | Self::Retain(subscription)
            | Self::Replace(subscription) => subscription.id(),
            Self::Cancel(id) => id,
        }
    }
}

pub(crate) fn erase_subscription_change<Message>(
    change: SubscriptionChange<Message>,
) -> ErasedSubscriptionChange
where
    Message: Send + 'static,
{
    match change {
        SubscriptionChange::Start { subscription } => {
            ErasedSubscriptionChange::Start(erase_subscription(subscription))
        }
        SubscriptionChange::Retain { subscription } => {
            ErasedSubscriptionChange::Retain(erase_subscription(subscription))
        }
        SubscriptionChange::Replace { subscription } => {
            ErasedSubscriptionChange::Replace(erase_subscription(subscription))
        }
        SubscriptionChange::Cancel { id } => ErasedSubscriptionChange::Cancel(id),
    }
}

#[derive(Clone)]
struct SourceStamp {
    component: ComponentId,
    subscription: SubscriptionId,
    generation: u64,
}

struct ActiveSource {
    stamp: SourceStamp,
    running: bool,
    subscription: Box<dyn ErasedRuntimeSubscription>,
    plan: SourcePlan,
}

enum DeliveryKind {
    Message,
    Timer,
}

struct QueuedMessage {
    target: ComponentId,
    target_message_type: TypeId,
    message_type_name: &'static str,
    message: ErasedValue,
    cause: TraceId,
    source: Option<SourceStamp>,
    delivery: DeliveryKind,
}

struct QueuedSourceEvent {
    stamp: SourceStamp,
    terminal_type: TypeId,
    event: ErasedSourceEvent,
    cause: TraceId,
}

enum ScheduledKind {
    Message(QueuedMessage),
    SourceEvent(QueuedSourceEvent),
}

struct ScheduledWork {
    deadline: Duration,
    ticket: u64,
    kind: ScheduledKind,
}

struct PendingEffectEntry {
    id: u64,
    component: ComponentId,
    descriptor_type: TypeId,
    descriptor_type_name: &'static str,
    descriptor: Option<ErasedValue>,
    mapper: Option<ErasedMessageMapper>,
    request_trace: TraceId,
    message_type: TypeId,
    message_type_name: &'static str,
}

struct OutstandingRequest {
    correlation: u64,
    requester: ComponentId,
    reply_type: TypeId,
    reply_type_name: &'static str,
    mapper: Option<ErasedMessageMapper>,
    message_type: TypeId,
    message_type_name: &'static str,
}

/// Exact and type-wide controlled behavior selected during assembly.
pub(crate) struct ControlledBindings {
    pub(crate) effects: HashSet<TypeId>,
    pub(crate) sources: HashSet<TypeId>,
    pub(crate) exact_sources: Vec<Box<dyn ErasedSourceDescriptor>>,
}

impl ControlledBindings {
    pub(crate) fn new() -> Self {
        Self {
            effects: HashSet::new(),
            sources: HashSet::new(),
            exact_sources: Vec::new(),
        }
    }
}

/// Owned mechanism behind the public [`crate::ControlledRuntime`].
pub(crate) struct ControlledCore {
    program: Program,
    bindings: ControlledBindings,
    now: Duration,
    next_ticket: u64,
    next_effect: u64,
    next_correlation: u64,
    next_generation: u64,
    scheduled: Vec<ScheduledWork>,
    effects: Vec<PendingEffectEntry>,
    requests: Vec<OutstandingRequest>,
    sources: Vec<ActiveSource>,
    trace: Vec<TraceRecord>,
    fault: Option<RuntimeError>,
}

impl ControlledCore {
    pub(crate) fn new(program: Program, bindings: ControlledBindings) -> Self {
        let mut runtime = Self {
            program,
            bindings,
            now: Duration::ZERO,
            next_ticket: 0,
            next_effect: 0,
            next_correlation: 0,
            next_generation: 0,
            scheduled: Vec::new(),
            effects: Vec::new(),
            requests: Vec::new(),
            sources: Vec::new(),
            trace: Vec::new(),
            fault: None,
        };
        runtime.initialize();
        runtime
    }

    fn initialize(&mut self) {
        let mut order = (0..self.program.components.len()).collect::<Vec<_>>();
        order.sort_by(|left, right| {
            self.program.components[*left]
                .id()
                .cmp(self.program.components[*right].id())
        });

        for index in order {
            if self.fault.is_some() {
                break;
            }
            let component = self.program.components[index].id().clone();
            let initialization = self.push_root(TraceEvent::Initialization {
                component: component.clone(),
            });
            let (changes, command) = {
                let kernel = &mut self.program.components[index];
                let changes = kernel.reconcile_subscriptions_erased();
                let command = kernel.take_initial_command_erased();
                (changes, command)
            };

            let changes = match changes {
                Ok(changes) => changes,
                Err(duplicate) => {
                    let _ = self.set_fault(
                        component,
                        None,
                        initialization,
                        format!("duplicate desired Subscription identity {duplicate:?}"),
                    );
                    break;
                }
            };
            if self
                .apply_subscription_changes(&component, changes, initialization)
                .is_err()
            {
                break;
            }
            if let Some(command) = command
                && command.interpret(self, &component, initialization).is_err()
            {
                break;
            }
        }
    }

    fn push_root(&mut self, event: TraceEvent) -> TraceId {
        self.push_trace(None, event)
    }

    fn push_child(&mut self, cause: TraceId, event: TraceEvent) -> TraceId {
        self.push_trace(Some(cause), event)
    }

    fn push_trace(&mut self, cause: Option<TraceId>, event: TraceEvent) -> TraceId {
        let id = TraceId(self.trace.len() as u64);
        self.trace.push(TraceRecord {
            id,
            at: LogicalTime(self.now),
            cause,
            event,
        });
        id
    }

    fn set_fault(
        &mut self,
        component: ComponentId,
        descriptor_type: Option<&'static str>,
        work: TraceId,
        reason: impl Into<Arc<str>>,
    ) -> RuntimeError {
        if let Some(error) = &self.fault {
            return error.clone();
        }
        let reason = reason.into();
        self.push_child(
            work,
            TraceEvent::RuntimeFault {
                component: component.clone(),
                work,
                descriptor_type: descriptor_type.unwrap_or("<none>"),
                reason: reason.clone(),
            },
        );
        let error = RuntimeError::fault(component, descriptor_type, work, reason);
        self.fault = Some(error.clone());
        error
    }

    fn ensure_not_faulted(&self) -> Result<(), RuntimeError> {
        match &self.fault {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    fn next_ticket(&mut self) -> u64 {
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        ticket
    }

    fn schedule(&mut self, deadline: Duration, kind: ScheduledKind) {
        let ticket = self.next_ticket();
        self.scheduled.push(ScheduledWork {
            deadline,
            ticket,
            kind,
        });
    }

    fn schedule_message(&mut self, message: QueuedMessage, deadline: Duration) {
        self.schedule(deadline, ScheduledKind::Message(message));
    }

    fn component_index(&self, id: &ComponentId, message_type: TypeId) -> Option<usize> {
        self.program.components.iter().position(|component| {
            component.id() == id && component.message_type_id() == message_type
        })
    }

    fn binding_index(&self, protocol: TypeId, port: &PortId) -> Option<usize> {
        self.program
            .bindings
            .iter()
            .position(|binding| binding.protocol_type() == protocol && binding.port_id() == port)
    }

    fn source_index(
        &self,
        component: &ComponentId,
        subscription: &SubscriptionId,
    ) -> Option<usize> {
        self.sources.iter().position(|source| {
            source.stamp.component == *component && source.stamp.subscription == *subscription
        })
    }

    fn source_stamp_is_current(&self, stamp: &SourceStamp) -> bool {
        self.source_index(&stamp.component, &stamp.subscription)
            .is_some_and(|index| self.sources[index].stamp.generation == stamp.generation)
    }

    fn terminal_is_controlled(&self, plan: &SourcePlan) -> bool {
        self.bindings.sources.contains(&plan.terminal_type_id())
            || self
                .bindings
                .exact_sources
                .iter()
                .any(|descriptor| descriptor.equals(plan.terminal_descriptor()))
    }

    fn allocate_generation(&mut self) -> u64 {
        let generation = self.next_generation;
        self.next_generation += 1;
        generation
    }

    fn activate_source(
        &mut self,
        component: &ComponentId,
        subscription: Box<dyn ErasedRuntimeSubscription>,
        work: TraceId,
    ) -> Result<(), RuntimeError> {
        let plan = subscription.source_plan();
        if !plan.accepts_output_event_type(subscription.source_event_type_id()) {
            return Err(self.set_fault(
                component.clone(),
                Some(plan.terminal_type_name()),
                work,
                "SourcePlan output does not match its Subscription event type",
            ));
        }
        if !self.terminal_is_controlled(&plan) {
            return Err(self.set_fault(
                component.clone(),
                Some(plan.terminal_type_name()),
                work,
                "missing controlled terminal Source behavior",
            ));
        }
        let generation = self.allocate_generation();
        self.sources.push(ActiveSource {
            stamp: SourceStamp {
                component: component.clone(),
                subscription: subscription.id().clone(),
                generation,
            },
            running: true,
            subscription,
            plan,
        });
        Ok(())
    }

    fn apply_subscription_changes(
        &mut self,
        component: &ComponentId,
        mut changes: Vec<ErasedSubscriptionChange>,
        cause: TraceId,
    ) -> Result<(), RuntimeError> {
        changes.sort_by(|left, right| left.id().cmp(right.id()));
        for change in changes {
            match change {
                ErasedSubscriptionChange::Start(subscription) => {
                    let work = self.push_child(
                        cause,
                        TraceEvent::SubscriptionLifecycle {
                            component: component.clone(),
                            subscription: subscription.id().clone(),
                            source_descriptor_type: subscription.descriptor_type_name(),
                            action: SubscriptionAction::Started,
                        },
                    );
                    self.activate_source(component, subscription, work)?;
                }
                ErasedSubscriptionChange::Retain(subscription) => {
                    self.push_child(
                        cause,
                        TraceEvent::SubscriptionLifecycle {
                            component: component.clone(),
                            subscription: subscription.id().clone(),
                            source_descriptor_type: subscription.descriptor_type_name(),
                            action: SubscriptionAction::Retained,
                        },
                    );
                    if let Some(index) = self.source_index(component, subscription.id()) {
                        // The SourcePlan and its Layer state stay intact. Only
                        // the latest declarative mapper/descriptor projection
                        // is installed.
                        self.sources[index].subscription = subscription;
                    }
                }
                ErasedSubscriptionChange::Replace(subscription) => {
                    let id = subscription.id().clone();
                    let work = self.push_child(
                        cause,
                        TraceEvent::SubscriptionLifecycle {
                            component: component.clone(),
                            subscription: id.clone(),
                            source_descriptor_type: subscription.descriptor_type_name(),
                            action: SubscriptionAction::Replaced,
                        },
                    );
                    if let Some(index) = self.source_index(component, &id) {
                        self.sources.remove(index);
                    }
                    self.activate_source(component, subscription, work)?;
                }
                ErasedSubscriptionChange::Cancel(id) => {
                    let source_descriptor_type = self
                        .source_index(component, &id)
                        .map(|index| self.sources[index].subscription.descriptor_type_name())
                        .unwrap_or("<inactive>");
                    self.push_child(
                        cause,
                        TraceEvent::SubscriptionLifecycle {
                            component: component.clone(),
                            subscription: id.clone(),
                            source_descriptor_type,
                            action: SubscriptionAction::Cancelled,
                        },
                    );
                    if let Some(index) = self.source_index(component, &id) {
                        self.sources.remove(index);
                    }
                }
            }
        }
        Ok(())
    }

    fn command_trace(
        &mut self,
        cause: TraceId,
        component: &ComponentId,
        kind: TraceCommandKind,
        detail_type: &'static str,
        target_component: Option<ComponentId>,
        target_port: Option<PortId>,
    ) -> TraceId {
        self.push_child(
            cause,
            TraceEvent::CommandEmitted {
                component: component.clone(),
                kind,
                detail_type,
                target_component,
                target_port,
            },
        )
    }

    fn interpret_command<Message>(
        &mut self,
        command: Command<Message>,
        component: &ComponentId,
        cause: TraceId,
    ) -> Result<(), RuntimeError>
    where
        Message: Send + 'static,
    {
        for declaration in command.into_declarations() {
            match declaration.0 {
                CommandKind::None | CommandKind::Batch(_) => {
                    unreachable!("Command::into_declarations removes None and Batch")
                }
                CommandKind::Effect(command) => {
                    let descriptor_type_name = command.intent_type_name();
                    let descriptor_type = command.intent().type_id();
                    let work = self.command_trace(
                        cause,
                        component,
                        TraceCommandKind::Effect,
                        descriptor_type_name,
                        None,
                        None,
                    );
                    if !self.bindings.effects.contains(&descriptor_type) {
                        return Err(self.set_fault(
                            component.clone(),
                            Some(descriptor_type_name),
                            work,
                            "missing controlled terminal Effect behavior",
                        ));
                    }
                    let (descriptor, mapper) = command.into_parts();
                    let mapper: ErasedMessageMapper =
                        Box::new(move |outcome| Box::new(mapper(outcome)));
                    let id = self.next_effect;
                    self.next_effect += 1;
                    self.effects.push(PendingEffectEntry {
                        id,
                        component: component.clone(),
                        descriptor_type,
                        descriptor_type_name,
                        descriptor: Some(descriptor),
                        mapper: Some(mapper),
                        request_trace: work,
                        message_type: TypeId::of::<Message>(),
                        message_type_name: std::any::type_name::<Message>(),
                    });
                }
                CommandKind::Send(command) => {
                    let target = command.target().clone();
                    let message_type = command.message_type_id();
                    let message_type_name = command.message_type_name();
                    let work = self.command_trace(
                        cause,
                        component,
                        TraceCommandKind::Send,
                        message_type_name,
                        Some(target.clone()),
                        None,
                    );
                    if !Arc::ptr_eq(command.target_program(), &self.program.program)
                        || self.component_index(&target, message_type).is_none()
                    {
                        return Err(self.set_fault(
                            component.clone(),
                            None,
                            work,
                            format!("direct send target {target:?} is absent from the Program"),
                        ));
                    }
                    self.schedule_message(
                        QueuedMessage {
                            target,
                            target_message_type: message_type,
                            message_type_name,
                            message: command.into_message(),
                            cause: work,
                            source: None,
                            delivery: DeliveryKind::Message,
                        },
                        self.now,
                    );
                }
                CommandKind::Notify(command) => {
                    let port = command.port().clone();
                    let protocol = command.protocol_type_id();
                    let detail_type = command.notification_type_name();
                    let work = self.command_trace(
                        cause,
                        component,
                        TraceCommandKind::Notify,
                        detail_type,
                        None,
                        Some(port.clone()),
                    );
                    if !Arc::ptr_eq(command.port_program(), &self.program.program) {
                        return Err(self.set_fault(
                            component.clone(),
                            None,
                            work,
                            "notification Port belongs to another Program",
                        ));
                    }
                    let Some(binding_index) = self.binding_index(protocol, &port) else {
                        return Err(self.set_fault(
                            component.clone(),
                            None,
                            work,
                            "notification Port has no runtime binding",
                        ));
                    };
                    let protocol_message = command.into_message();
                    let (target, message_type, message_type_name, message) = {
                        let binding = &self.program.bindings[binding_index];
                        (
                            binding.provider_id().clone(),
                            binding.provider_message_type(),
                            binding.provider_message_type_name(),
                            binding.convert(protocol_message),
                        )
                    };
                    self.schedule_message(
                        QueuedMessage {
                            target,
                            target_message_type: message_type,
                            message_type_name,
                            message,
                            cause: work,
                            source: None,
                            delivery: DeliveryKind::Message,
                        },
                        self.now,
                    );
                }
                CommandKind::Request(command) => {
                    let port = command.port().clone();
                    let protocol = command.protocol_type_id();
                    let request_type = command.request_type_name();
                    let reply_type = command.reply_type_id();
                    let reply_type_name = command.reply_type_name();
                    let work = self.command_trace(
                        cause,
                        component,
                        TraceCommandKind::Request,
                        request_type,
                        None,
                        Some(port.clone()),
                    );
                    if !Arc::ptr_eq(command.port_program(), &self.program.program) {
                        return Err(self.set_fault(
                            component.clone(),
                            None,
                            work,
                            "request Port belongs to another Program",
                        ));
                    }
                    let Some(binding_index) = self.binding_index(protocol, &port) else {
                        return Err(self.set_fault(
                            component.clone(),
                            None,
                            work,
                            "request Port has no runtime binding",
                        ));
                    };
                    let correlation = self.next_correlation;
                    self.next_correlation += 1;
                    let (protocol_message, mapper): (_, ErasedRequestMapper<Message>) =
                        command.into_parts(correlation);
                    let mapper: ErasedMessageMapper =
                        Box::new(move |reply| Box::new(mapper(reply)));
                    let (target, message_type, message_type_name, message) = {
                        let binding = &self.program.bindings[binding_index];
                        (
                            binding.provider_id().clone(),
                            binding.provider_message_type(),
                            binding.provider_message_type_name(),
                            binding.convert(protocol_message),
                        )
                    };
                    self.requests.push(OutstandingRequest {
                        correlation,
                        requester: component.clone(),
                        reply_type,
                        reply_type_name,
                        mapper: Some(mapper),
                        message_type: TypeId::of::<Message>(),
                        message_type_name: std::any::type_name::<Message>(),
                    });
                    self.schedule_message(
                        QueuedMessage {
                            target,
                            target_message_type: message_type,
                            message_type_name,
                            message,
                            cause: work,
                            source: None,
                            delivery: DeliveryKind::Message,
                        },
                        self.now,
                    );
                }
                CommandKind::Reply(command) => {
                    let reply_type = command.reply_type_id();
                    let reply_type_name = command.reply_type_name();
                    let work = self.command_trace(
                        cause,
                        component,
                        TraceCommandKind::Reply,
                        reply_type_name,
                        None,
                        None,
                    );
                    let (correlation, reply) = command.into_parts();
                    let Some(index) = self
                        .requests
                        .iter()
                        .position(|request| request.correlation == correlation)
                    else {
                        return Err(self.set_fault(
                            component.clone(),
                            None,
                            work,
                            "ReplyTo no longer names an outstanding Request",
                        ));
                    };
                    let mut request = self.requests.remove(index);
                    if request.reply_type != reply_type {
                        return Err(self.set_fault(
                            component.clone(),
                            None,
                            work,
                            "Reply type does not match its Request correlation",
                        ));
                    }
                    let outcome = self.push_child(
                        work,
                        TraceEvent::RequestOutcome {
                            component: request.requester.clone(),
                            reply_type: request.reply_type_name,
                        },
                    );
                    let mapper = request
                        .mapper
                        .take()
                        .expect("an outstanding Request owns one mapper");
                    self.schedule_message(
                        QueuedMessage {
                            target: request.requester,
                            target_message_type: request.message_type,
                            message_type_name: request.message_type_name,
                            message: mapper(reply),
                            cause: outcome,
                            source: None,
                            delivery: DeliveryKind::Message,
                        },
                        self.now,
                    );
                }
                CommandKind::After { delay, message } => {
                    let message_type_name = std::any::type_name::<Message>();
                    let work = self.command_trace(
                        cause,
                        component,
                        TraceCommandKind::Timer,
                        message_type_name,
                        Some(component.clone()),
                        None,
                    );
                    let Some(deadline) = self.now.checked_add(delay) else {
                        return Err(self.set_fault(
                            component.clone(),
                            None,
                            work,
                            "logical timer deadline exceeds the Duration range",
                        ));
                    };
                    self.schedule_message(
                        QueuedMessage {
                            target: component.clone(),
                            target_message_type: TypeId::of::<Message>(),
                            message_type_name,
                            message: Box::new(message),
                            cause: work,
                            source: None,
                            delivery: DeliveryKind::Timer,
                        },
                        deadline,
                    );
                }
            }
        }
        Ok(())
    }

    pub(crate) fn send<C: Component>(
        &mut self,
        component: &ComponentRef<C>,
        message: C::Message,
    ) -> Result<(), RuntimeError> {
        self.ensure_not_faulted()?;
        if !Arc::ptr_eq(&component.program, &self.program.program)
            || self
                .component_index(component.id(), TypeId::of::<C::Message>())
                .is_none()
        {
            return Err(RuntimeError::harness(
                "controlled Message target is not part of this Program",
            ));
        }
        let cause = self.push_root(TraceEvent::ControlledMessageInput {
            component: component.id().clone(),
            message_type: std::any::type_name::<C::Message>(),
        });
        self.schedule_message(
            QueuedMessage {
                target: component.id().clone(),
                target_message_type: TypeId::of::<C::Message>(),
                message_type_name: std::any::type_name::<C::Message>(),
                message: Box::new(message),
                cause,
                source: None,
                delivery: DeliveryKind::Message,
            },
            self.now,
        );
        Ok(())
    }

    fn enqueue_source_event<S: SourceDescriptor>(
        &mut self,
        source_index: usize,
        event: SourceEvent<S::Item, S::Error>,
    ) -> Result<(), RuntimeError> {
        let source = &self.sources[source_index];
        if !source.running || source.plan.terminal_type_id() != TypeId::of::<S>() {
            return Err(RuntimeError::harness(
                "controlled Source input does not match an active terminal descriptor",
            ));
        }
        let stamp = source.stamp.clone();
        let terminal_type_name = source.plan.terminal_type_name();
        let kind = SourceEventKind::of(&event);
        let cause = self.push_root(TraceEvent::ControlledSourceInput {
            component: stamp.component.clone(),
            subscription: stamp.subscription.clone(),
            source_descriptor_type: terminal_type_name,
            event: kind,
        });
        self.schedule(
            self.now,
            ScheduledKind::SourceEvent(QueuedSourceEvent {
                stamp,
                terminal_type: TypeId::of::<S>(),
                event: ErasedSourceEvent::typed::<S>(event),
                cause,
            }),
        );
        Ok(())
    }

    pub(crate) fn emit_stream<T: Send + 'static>(
        &mut self,
        stream: &StreamDescriptor<T>,
        item: T,
    ) -> Result<(), RuntimeError> {
        self.ensure_not_faulted()?;
        let matches = self
            .sources
            .iter()
            .enumerate()
            .filter_map(|(index, source)| {
                (source.running
                    && source.plan.terminal_type_id() == TypeId::of::<StreamDescriptor<T>>()
                    && source
                        .plan
                        .terminal_descriptor()
                        .downcast_ref::<StreamDescriptor<T>>()
                        == Some(stream))
                .then_some(index)
            })
            .collect::<Vec<_>>();
        let [index] = matches.as_slice() else {
            return Err(RuntimeError::harness(
                "controlled stream input requires exactly one active matching Source",
            ));
        };
        self.enqueue_source_event::<StreamDescriptor<T>>(*index, SourceEvent::Item(item))
    }

    pub(crate) fn close_stream<T: Send + 'static>(
        &mut self,
        stream: &StreamDescriptor<T>,
    ) -> Result<(), RuntimeError> {
        self.ensure_not_faulted()?;
        let matches = self
            .sources
            .iter()
            .enumerate()
            .filter_map(|(index, source)| {
                (source.running
                    && source.plan.terminal_type_id() == TypeId::of::<StreamDescriptor<T>>()
                    && source
                        .plan
                        .terminal_descriptor()
                        .downcast_ref::<StreamDescriptor<T>>()
                        == Some(stream))
                .then_some(index)
            })
            .collect::<Vec<_>>();
        let [index] = matches.as_slice() else {
            return Err(RuntimeError::harness(
                "controlled stream close requires exactly one active matching Source",
            ));
        };
        self.enqueue_source_event::<StreamDescriptor<T>>(*index, SourceEvent::Ended)
    }

    pub(crate) fn emit_source<C, S>(
        &mut self,
        component: &ComponentRef<C>,
        id: &SubscriptionId,
        event: SourceEvent<S::Item, S::Error>,
    ) -> Result<(), RuntimeError>
    where
        C: Component,
        S: SourceDescriptor,
    {
        self.ensure_not_faulted()?;
        if !Arc::ptr_eq(&component.program, &self.program.program) {
            return Err(RuntimeError::harness(
                "controlled Source target is not part of this Program",
            ));
        }
        let Some(index) = self.source_index(component.id(), id) else {
            return Err(RuntimeError::harness(
                "controlled Source input targets no active Subscription",
            ));
        };
        self.enqueue_source_event::<S>(index, event)
    }

    pub(crate) fn source_descriptor<C, S>(
        &self,
        component: &ComponentRef<C>,
        id: &SubscriptionId,
    ) -> Result<S, RuntimeError>
    where
        C: Component,
        S: SourceDescriptor,
    {
        if !Arc::ptr_eq(&component.program, &self.program.program) {
            return Err(RuntimeError::harness(
                "controlled Source target is not part of this Program",
            ));
        }
        let Some(index) = self.source_index(component.id(), id) else {
            return Err(RuntimeError::harness("no active Source for Subscription"));
        };
        if !self.sources[index].running {
            return Err(RuntimeError::harness("Source is no longer active"));
        }
        self.sources[index]
            .subscription
            .descriptor()
            .downcast_ref::<S>()
            .cloned()
            .ok_or_else(|| RuntimeError::harness("active SourceDescriptor type does not match"))
    }

    pub(crate) fn next_effect<E: EffectDescriptor>(
        &mut self,
    ) -> Result<PendingEffect<E>, RuntimeError> {
        self.ensure_not_faulted()?;
        let Some(index) = self.effects.iter().position(|effect| {
            effect.descriptor_type == TypeId::of::<E>() && effect.descriptor.is_some()
        }) else {
            return Err(RuntimeError::harness(
                "no pending effect of the requested descriptor type",
            ));
        };
        let descriptor = self.effects[index]
            .descriptor
            .take()
            .expect("the selected effect still owns its descriptor");
        let intent = match descriptor.downcast::<E>() {
            Ok(intent) => *intent,
            Err(_) => unreachable!("pending effect TypeId and descriptor agree"),
        };
        Ok(PendingEffect {
            intent,
            id: self.effects[index].id,
            program: self.program.program.clone(),
        })
    }

    pub(crate) fn complete<E: EffectDescriptor>(
        &mut self,
        pending: PendingEffect<E>,
        outcome: EffectOutcome<E::Output, E::Error>,
    ) -> Result<(), RuntimeError> {
        self.ensure_not_faulted()?;
        if !Arc::ptr_eq(&pending.program, &self.program.program) {
            return Err(RuntimeError::harness(
                "pending effect belongs to another controlled runtime",
            ));
        }
        let Some(index) = self.effects.iter().position(|effect| {
            effect.id == pending.id && effect.descriptor_type == TypeId::of::<E>()
        }) else {
            return Err(RuntimeError::harness(
                "pending effect is no longer owned by this runtime",
            ));
        };
        let mut effect = self.effects.remove(index);
        let outcome_kind = match &outcome {
            EffectOutcome::Succeeded(_) => EffectOutcomeKind::Succeeded,
            EffectOutcome::Failed(_) => EffectOutcomeKind::Failed,
            EffectOutcome::Cancelled(_) => EffectOutcomeKind::Cancelled,
        };
        let outcome_trace = self.push_root(TraceEvent::EffectOutcome {
            effect: effect.request_trace,
            component: effect.component.clone(),
            effect_type: effect.descriptor_type_name,
            outcome: outcome_kind,
        });
        let mapper = effect
            .mapper
            .take()
            .expect("a pending effect owns one mapper");
        self.schedule_message(
            QueuedMessage {
                target: effect.component,
                target_message_type: effect.message_type,
                message_type_name: effect.message_type_name,
                message: mapper(Box::new(outcome)),
                cause: outcome_trace,
                source: None,
                delivery: DeliveryKind::Message,
            },
            self.now,
        );
        Ok(())
    }

    fn pop_next_due(&mut self) -> Option<ScheduledWork> {
        let index = self
            .scheduled
            .iter()
            .enumerate()
            .filter(|(_, work)| work.deadline <= self.now)
            .min_by_key(|(_, work)| (work.deadline, work.ticket))
            .map(|(index, _)| index)?;
        Some(self.scheduled.remove(index))
    }

    fn process_source_event(&mut self, work: QueuedSourceEvent) -> Result<usize, RuntimeError> {
        let Some(index) = self.source_index(&work.stamp.component, &work.stamp.subscription) else {
            self.push_child(
                work.cause,
                TraceEvent::StaleSourceWorkDropped {
                    component: work.stamp.component,
                    subscription: work.stamp.subscription,
                    mapped_message: false,
                },
            );
            return Ok(0);
        };
        if self.sources[index].stamp.generation != work.stamp.generation
            || !self.sources[index].running
            || self.sources[index].plan.terminal_type_id() != work.terminal_type
        {
            self.push_child(
                work.cause,
                TraceEvent::StaleSourceWorkDropped {
                    component: work.stamp.component,
                    subscription: work.stamp.subscription,
                    mapped_message: false,
                },
            );
            return Ok(0);
        }

        let (outer_type, message_type, message_type_name, mapped) = {
            let source = &mut self.sources[index];
            let outer_type = source.subscription.descriptor_type_name();
            let message_type = source.subscription.message_type_id();
            let message_type_name = source.subscription.message_type_name();
            let raw_terminal = work.event.kind.is_terminal();
            let outer_events = source.plan.map_event(work.event);
            let terminal =
                raw_terminal || outer_events.iter().any(|event| event.kind.is_terminal());
            let mapped = outer_events
                .into_iter()
                .map(|event| {
                    let kind = event.kind;
                    let message = source.subscription.map_event(event);
                    (kind, message)
                })
                .collect::<Vec<_>>();
            if terminal {
                source.running = false;
            }
            (outer_type, message_type, message_type_name, mapped)
        };

        for (kind, message) in mapped {
            let mapped_trace = self.push_child(
                work.cause,
                TraceEvent::SourceEventMapped {
                    component: work.stamp.component.clone(),
                    subscription: work.stamp.subscription.clone(),
                    source_descriptor_type: outer_type,
                    event: kind,
                },
            );
            self.schedule_message(
                QueuedMessage {
                    target: work.stamp.component.clone(),
                    target_message_type: message_type,
                    message_type_name,
                    message,
                    cause: mapped_trace,
                    source: Some(work.stamp.clone()),
                    delivery: DeliveryKind::Message,
                },
                self.now,
            );
        }
        Ok(0)
    }

    fn process_message(&mut self, message: QueuedMessage) -> Result<usize, RuntimeError> {
        if let Some(stamp) = &message.source
            && !self.source_stamp_is_current(stamp)
        {
            self.push_child(
                message.cause,
                TraceEvent::StaleSourceWorkDropped {
                    component: stamp.component.clone(),
                    subscription: stamp.subscription.clone(),
                    mapped_message: true,
                },
            );
            return Ok(0);
        }

        let delivery_cause = match message.delivery {
            DeliveryKind::Message => message.cause,
            DeliveryKind::Timer => self.push_child(
                message.cause,
                TraceEvent::TimerFired {
                    component: message.target.clone(),
                    message_type: message.message_type_name,
                },
            ),
        };

        let Some(index) = self.component_index(&message.target, message.target_message_type) else {
            return Err(self.set_fault(
                message.target,
                None,
                delivery_cause,
                "Message target is absent or has a different Message type",
            ));
        };
        let command = {
            let kernel = &mut self.program.components[index];
            kernel
                .transition_erased(message.message)
                .map_err(|_| RuntimeError::harness("internal typed Message delivery mismatch"))?
        };
        let transition = self.push_child(
            delivery_cause,
            TraceEvent::Transition {
                component: message.target.clone(),
                message_type: message.message_type_name,
            },
        );
        let changes = {
            let kernel = &mut self.program.components[index];
            kernel.reconcile_subscriptions_erased()
        };
        let changes = match changes {
            Ok(changes) => changes,
            Err(duplicate) => {
                return Err(self.set_fault(
                    message.target,
                    None,
                    transition,
                    format!("duplicate desired Subscription identity {duplicate:?}"),
                ));
            }
        };
        self.apply_subscription_changes(&message.target, changes, transition)?;
        command.interpret(self, &message.target, transition)?;
        Ok(1)
    }

    pub(crate) fn run_until_idle(&mut self) -> Result<RunReport, RuntimeError> {
        self.ensure_not_faulted()?;
        let mut transitions = 0;
        while let Some(work) = self.pop_next_due() {
            transitions += match work.kind {
                ScheduledKind::Message(message) => self.process_message(message)?,
                ScheduledKind::SourceEvent(event) => self.process_source_event(event)?,
            };
            self.ensure_not_faulted()?;
        }
        let pending = self.pending_work();
        Ok(RunReport {
            transitions,
            pending_now: pending.pending_now,
            pending_later: pending.pending_later,
        })
    }

    pub(crate) fn advance(&mut self, duration: Duration) -> Result<RunReport, RuntimeError> {
        self.ensure_not_faulted()?;
        let Some(target) = self.now.checked_add(duration) else {
            return Err(RuntimeError::harness(
                "logical time advance exceeds the Duration range",
            ));
        };
        let mut transitions = self.run_until_idle()?.transitions;
        loop {
            let next = self
                .scheduled
                .iter()
                .filter(|work| work.deadline > self.now && work.deadline <= target)
                .map(|work| work.deadline)
                .min();
            let Some(next) = next else {
                break;
            };
            self.now = next;
            transitions += self.run_until_idle()?.transitions;
        }
        self.now = target;
        transitions += self.run_until_idle()?.transitions;
        let pending = self.pending_work();
        Ok(RunReport {
            transitions,
            pending_now: pending.pending_now,
            pending_later: pending.pending_later,
        })
    }

    pub(crate) fn advance_to_next(&mut self) -> Result<RunReport, RuntimeError> {
        self.ensure_not_faulted()?;
        let mut transitions = self.run_until_idle()?.transitions;
        let Some(next) = self
            .scheduled
            .iter()
            .filter(|work| work.deadline > self.now)
            .map(|work| work.deadline)
            .min()
        else {
            return Err(RuntimeError::harness("no future controlled work exists"));
        };
        self.now = next;
        transitions += self.run_until_idle()?.transitions;
        let pending = self.pending_work();
        Ok(RunReport {
            transitions,
            pending_now: pending.pending_now,
            pending_later: pending.pending_later,
        })
    }

    pub(crate) fn pending_work(&self) -> PendingWork {
        let (pending_now, future_timers) =
            self.scheduled
                .iter()
                .fold((0, 0), |(pending_now, pending_later), work| {
                    match &work.kind {
                        ScheduledKind::SourceEvent(_) => (pending_now, pending_later),
                        ScheduledKind::Message(_) if work.deadline <= self.now => {
                            (pending_now + 1, pending_later)
                        }
                        ScheduledKind::Message(_) => (pending_now, pending_later + 1),
                    }
                });
        PendingWork {
            pending_now,
            pending_later: future_timers
                + self.effects.len()
                + self.sources.iter().filter(|source| source.running).count()
                + self.requests.len(),
        }
    }

    pub(crate) fn cancel(mut self) -> Result<ShutdownReport, RuntimeError> {
        let pending = self.pending_work();
        let cancelled = pending.pending_now + pending.pending_later;
        self.scheduled.clear();
        self.effects.clear();
        self.sources.clear();
        self.requests.clear();
        Ok(ShutdownReport {
            completed: 0,
            cancelled,
            remaining: 0,
            pending_now: 0,
            pending_later: 0,
        })
    }

    pub(crate) fn state<C: Component>(
        &self,
        component: &ComponentRef<C>,
    ) -> Result<&C::Model, RuntimeError> {
        if !Arc::ptr_eq(&component.program, &self.program.program) {
            return Err(RuntimeError::harness(
                "controlled state target is not part of this Program",
            ));
        }
        self.program
            .components
            .iter()
            .find(|kernel| {
                kernel.id() == component.id() && kernel.component_type_id() == TypeId::of::<C>()
            })
            .and_then(|kernel| kernel.model_any().downcast_ref::<C::Model>())
            .ok_or_else(|| RuntimeError::harness("controlled state target type does not match"))
    }

    pub(crate) fn trace(&self) -> &[TraceRecord] {
        &self.trace
    }
}

pub(crate) fn exact_source<S: SourceDescriptor>(descriptor: S) -> Box<dyn ErasedSourceDescriptor> {
    Box::new(SourceDescriptorSnapshot(descriptor))
}
