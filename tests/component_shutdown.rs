//! Acceptance evidence for `Component::shutdown` (ADR-0024): what Drain does
//! once the Program is quiet, in what order, what a stopping Component may
//! do, and what its peers hear.

use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use samara::prelude::*;

/// A last write a Component makes as it stops.
#[derive(Debug, PartialEq)]
struct Farewell(&'static str);

impl EffectDescriptor for Farewell {
    type Output = ();
    type Error = Infallible;
}

/// Holds reply tokens until it is told to answer, or until it stops.
struct Vault;

#[derive(Debug)]
enum VaultMessage {
    /// Hold this caller's token.
    Hold(ReplyWith<&'static str>),
    /// Hold this caller's token and drop it at shutdown without answering.
    Forget(ReplyWith<&'static str>),
    /// A peer's parting note.
    Note(&'static str),
}

#[derive(Default)]
struct VaultModel {
    held: Vec<ReplyWith<&'static str>>,
    forgotten: Vec<ReplyWith<&'static str>>,
    notes: Vec<&'static str>,
}

impl Component for Vault {
    type Requires = Effect<Farewell>;
    type Config = ();
    type Model = VaultModel;
    type Message = VaultMessage;

    fn init(_: &(), _: &Capabilities<Self::Requires>) -> Init<VaultModel, VaultMessage> {
        Init::default()
    }

    fn update(
        _: &(),
        _: &Capabilities<Self::Requires>,
        model: &mut VaultModel,
        message: VaultMessage,
    ) -> Command<VaultMessage> {
        match message {
            VaultMessage::Hold(reply) => model.held.push(reply),
            VaultMessage::Forget(reply) => model.forgotten.push(reply),
            VaultMessage::Note(note) => model.notes.push(note),
        }
        Command::none()
    }

    /// Answers every held caller and writes one farewell. The forgotten
    /// tokens are dropped with the Model.
    fn shutdown(
        _: &(),
        farewell: &Capabilities<Self::Requires>,
        model: VaultModel,
    ) -> Command<NoMessage> {
        let mut finishing: Vec<Command<NoMessage>> = model
            .held
            .into_iter()
            .map(|reply| Command::reply(reply, "closing"))
            .collect();
        finishing.push(Effect::perform(farewell, Farewell("vault")).discard_outcome());
        Command::batch(finishing)
    }
}

#[derive(Debug, PartialEq)]
enum AskerMessage {
    Hold,
    Forget,
    Heard(RequestOutcome<&'static str>),
}

fn ask<T>(
    vault: &T,
    heard: &mut Vec<RequestOutcome<&'static str>>,
    message: AskerMessage,
) -> Command<AskerMessage>
where
    T: Addressable<Message = VaultMessage>,
{
    match message {
        AskerMessage::Hold => {
            Command::request(vault, VaultMessage::Hold).producing(AskerMessage::Heard)
        }
        AskerMessage::Forget => {
            Command::request(vault, VaultMessage::Forget).producing(AskerMessage::Heard)
        }
        AskerMessage::Heard(outcome) => {
            heard.push(outcome);
            Command::none()
        }
    }
}

/// Registered before the vault, so it stops after it and hears how the vault
/// resolved its requests. It reaches the vault through a Port, which is what
/// lets it be registered first.
struct Elder;

impl Component for Elder {
    type Requires = ();
    type Config = Port<VaultMessage>;
    type Model = Vec<RequestOutcome<&'static str>>;
    type Message = AskerMessage;

    fn init(_: &Self::Config, _: &()) -> Init<Self::Model, AskerMessage> {
        Init::default()
    }

    fn update(
        vault: &Self::Config,
        _: &(),
        heard: &mut Self::Model,
        message: AskerMessage,
    ) -> Command<AskerMessage> {
        ask(vault, heard, message)
    }
}

/// Registered after the vault, so it stops first and its parting note lands
/// on a vault that is still alive.
struct Asker;

impl Component for Asker {
    type Requires = ();
    type Config = Connector<VaultMessage>;
    type Model = Vec<RequestOutcome<&'static str>>;
    type Message = AskerMessage;

    fn init(_: &Self::Config, _: &()) -> Init<Self::Model, AskerMessage> {
        Init::default()
    }

    fn update(
        vault: &Self::Config,
        _: &(),
        heard: &mut Self::Model,
        message: AskerMessage,
    ) -> Command<AskerMessage> {
        ask(vault, heard, message)
    }

    fn shutdown(vault: &Self::Config, _: &(), _: Self::Model) -> Command<NoMessage> {
        Command::send(vault, VaultMessage::Note("asker out"))
    }
}

type Needs = <Vault as Component>::Requires;

struct Fixture {
    program: Program<Needs>,
    elder: ComponentRef<Elder>,
    vault: ComponentRef<Vault>,
    asker: ComponentRef<Asker>,
    vault_port: Port<VaultMessage>,
}

/// Registered in the order elder, vault, asker. Drain stops them in reverse:
/// asker, vault, elder.
fn fixture() -> Fixture {
    let (mut builder, capabilities) = Program::builder::<Needs>();
    let vault_port = builder.port::<VaultMessage>(PortId::new("vault"));
    let elder = builder
        .component::<Elder>(vault_port.clone())
        .finish()
        .unwrap();
    let vault = builder
        .component::<Vault>(())
        .with_capabilities(capabilities)
        .finish()
        .unwrap();
    builder.connect(&vault_port, &vault.connector());
    let asker = builder
        .component::<Asker>(vault.connector())
        .finish()
        .unwrap();
    Fixture {
        program: builder.build().unwrap(),
        elder,
        vault,
        asker,
        vault_port,
    }
}

fn controlled(program: Program<Needs>) -> ControlledRuntime {
    ControlledRuntime::builder(program)
        .control_all()
        .build()
        .unwrap()
}

fn shutdown_order(runtime: &ControlledRuntime) -> Vec<ComponentId> {
    runtime
        .trace()
        .iter()
        .filter_map(|record| match &record.event {
            TraceEvent::ComponentShutdown { component } => Some(component.clone()),
            _ => None,
        })
        .collect()
}

fn position(runtime: &ControlledRuntime, wanted: impl Fn(&TraceEvent) -> bool) -> usize {
    runtime
        .trace()
        .iter()
        .position(|record| wanted(&record.event))
        .expect("the event is in the trace")
}

#[test]
fn drain_stops_components_in_reverse_registration_order_awaiting_finishing_effects() {
    let Fixture {
        program,
        elder,
        vault,
        asker,
        ..
    } = fixture();
    let mut runtime = controlled(program);
    runtime.run_until_idle().unwrap();

    runtime.drain().unwrap();
    // The vault's farewell is in flight, so the elder has not been stopped.
    assert_eq!(
        shutdown_order(&runtime),
        [asker.id().clone(), vault.id().clone()]
    );
    let farewell = runtime.next_effect::<Farewell>().unwrap();
    assert_eq!(farewell.intent, Farewell("vault"));
    runtime
        .complete(farewell, EffectOutcome::Succeeded(()))
        .unwrap();
    runtime.run_until_idle().unwrap();

    assert_eq!(
        shutdown_order(&runtime),
        [asker.id().clone(), vault.id().clone(), elder.id().clone()]
    );
    let report = runtime.cancel().unwrap();
    assert!(report.is_clean());
    assert!(report.not_shut_down.is_empty());
}

#[test]
fn a_finishing_send_lands_on_a_peer_that_is_still_alive() {
    let Fixture {
        program,
        vault,
        asker,
        ..
    } = fixture();
    let mut runtime = controlled(program);
    runtime.run_until_idle().unwrap();
    runtime.drain().unwrap();

    // The asker's parting note was an ordinary transition on the vault,
    // after the asker stopped and before the vault did.
    let asker_stop = position(
        &runtime,
        |event| matches!(event, TraceEvent::ComponentShutdown { component } if component == asker.id()),
    );
    let note = position(
        &runtime,
        |event| matches!(event, TraceEvent::Transition { component, .. } if component == vault.id()),
    );
    let vault_stop = position(
        &runtime,
        |event| matches!(event, TraceEvent::ComponentShutdown { component } if component == vault.id()),
    );
    assert!(asker_stop < note && note < vault_stop);
    assert!(
        runtime.state(&vault).is_err(),
        "a stopped Component has no Model"
    );
}

#[test]
fn shutdown_answers_held_callers_and_abandons_dropped_tokens() {
    let Fixture { program, elder, .. } = fixture();
    let mut runtime = controlled(program);
    runtime.send(&elder, AskerMessage::Hold).unwrap();
    runtime.send(&elder, AskerMessage::Forget).unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.state(&elder).unwrap().is_empty());
    // Two requests are outstanding and nothing else is pending. Before
    // ADR-0024 this Drain would have waited forever.
    assert_eq!(runtime.pending_work().pending_later, 2);

    runtime.drain().unwrap();
    // The vault stopped before the elder: it answered the token it held and
    // dropped the other with its Model. The elder heard both as ordinary
    // transitions and is still alive, because the vault's farewell is pending.
    assert_eq!(
        runtime.state(&elder).unwrap(),
        &[
            RequestOutcome::Replied("closing"),
            RequestOutcome::Failed(RequestError::ReplyAbandoned),
        ]
    );
    let outcomes: Vec<_> = runtime
        .trace()
        .iter()
        .filter_map(|record| match &record.event {
            TraceEvent::RequestOutcome { outcome, .. } => Some(*outcome),
            _ => None,
        })
        .collect();
    assert_eq!(
        outcomes,
        [RequestOutcomeKind::Replied, RequestOutcomeKind::Abandoned]
    );
}

#[test]
fn a_message_to_a_stopped_component_is_dropped_and_traced() {
    let Fixture { program, asker, .. } = fixture();
    let mut runtime = controlled(program);
    runtime.run_until_idle().unwrap();
    runtime.drain().unwrap();
    assert_eq!(shutdown_order(&runtime).first(), Some(asker.id()));

    runtime.send(&asker, AskerMessage::Hold).unwrap();
    assert_eq!(runtime.run_until_idle().unwrap().transitions, 0);
    assert!(runtime.trace().iter().any(|record| matches!(
        &record.event,
        TraceEvent::MessageDropped { component, .. } if component == asker.id()
    )));
}

#[test]
fn cancel_calls_no_shutdown_and_a_cut_short_drain_names_who_missed_it() {
    let Fixture { program, .. } = fixture();
    let mut runtime = controlled(program);
    runtime.run_until_idle().unwrap();
    assert!(shutdown_order(&runtime).is_empty());
    let report = runtime.cancel().unwrap();
    assert!(
        report.not_shut_down.is_empty(),
        "a plain Cancel names nobody"
    );

    let Fixture { program, elder, .. } = fixture();
    let mut runtime = controlled(program);
    runtime.run_until_idle().unwrap();
    runtime.drain().unwrap();
    // The vault's farewell is still in flight, so the elder has not had its
    // step. Cancelling now is what an escalation does.
    let report = runtime.cancel().unwrap();
    assert_eq!(report.not_shut_down, [elder.id().clone()]);
}

struct RecordingFarewell(Arc<Mutex<Vec<&'static str>>>);

impl EffectDriver<Farewell> for RecordingFarewell {
    fn execute(&self, farewell: Farewell) -> BoxFuture<Result<(), Infallible>> {
        let written = self.0.clone();
        Box::pin(async move {
            written.lock().unwrap().push(farewell.0);
            Ok(())
        })
    }
}

#[tokio::test]
async fn live_drain_completes_when_a_provider_holds_a_reply_token() -> Result<(), RuntimeError> {
    let Fixture {
        program,
        vault_port,
        ..
    } = fixture();
    let written = Arc::new(Mutex::new(Vec::new()));
    let runtime = Runtime::builder(program)
        .with_drivers(RecordingFarewell(written.clone()))
        .build()?;
    let vault = runtime.port_handle(&vault_port)?;
    let task = runtime.spawn();

    let held = tokio::spawn({
        let vault = vault.clone();
        async move { vault.request_to(VaultMessage::Hold).await }
    });
    let forgotten = tokio::spawn({
        let vault = vault.clone();
        async move { vault.request_to(VaultMessage::Forget).await }
    });
    // Let both requests be admitted and reach the vault before draining.
    while vault
        .request_to_timeout(VaultMessage::Forget, std::time::Duration::from_millis(5))
        .await?
        != RequestOutcome::TimedOut
    {}

    // Before ADR-0024 this Drain never finished: the vault holds tokens.
    assert!(task.shutdown(Shutdown::Drain).await?.is_clean());
    assert_eq!(held.await.unwrap()?, "closing");
    assert!(
        forgotten.await.unwrap().is_err(),
        "an abandoned host request ends with an error"
    );
    assert_eq!(*written.lock().unwrap(), ["vault"]);
    Ok(())
}
