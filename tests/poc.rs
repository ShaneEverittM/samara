use std::time::Duration;

mod support;

use samara::{
    effects::TokioBackend,
    runtime::{
        Actor, ActorId, AskError, DeadLetterReason, Envelope, RegisterError, RegisterPortError,
        RunUntil, RunUntilExit, Runtime, RuntimeAskError, RuntimeTellError,
    },
};
use support::{
    adder::{AdderActor, AdderModel, AdderMsg},
    counter::{self, CounterActor, CounterModel, CounterMsg},
    ports::{
        AccumulatorPort, AccumulatorReq, AccumulatorRes, MockAccumulatorActor, RealAccumulatorActor,
    },
    store::InMemoryStore,
};

#[test]
fn update_is_deterministic() {
    let mut first_model = CounterModel::default();
    let mut second_model = CounterModel::default();
    let msg = CounterMsg::IncrementRequested;

    let first_cmds = counter::update(&mut first_model, msg.clone());
    let second_cmds = counter::update(&mut second_model, msg);

    assert_eq!(first_model, second_model);
    assert_eq!(first_cmds, second_cmds);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_can_drive_counter_actor_with_two_effects() {
    let store = InMemoryStore::default();
    let mut runtime = Runtime::new(128, std::sync::Arc::new(TokioBackend));

    let counter_id = ActorId(1);
    let counter_addr = runtime
        .register_actor(
            counter_id,
            CounterActor::new(CounterModel::default()),
            CounterActor::effect_driver(store.clone()),
        )
        .expect("counter id must be unique");

    counter_addr
        .send(CounterMsg::IncrementRequested)
        .await
        .expect("mailbox should be open");

    let exit = runtime.run_until_idle().await;
    assert_eq!(exit, RunUntilExit::Idle);

    let model = runtime
        .actor::<CounterActor>(counter_id)
        .expect("counter should be registered")
        .lock()
        .await
        .model()
        .clone();

    assert_eq!(model.count, 1);
    assert_eq!(model.persisted_count, Some(1));
    assert_eq!(model.ticks, 1);
    assert!(model.last_error.is_none());

    let persisted_values = store.snapshot().await;
    assert_eq!(persisted_values, vec![1]);
    assert!(runtime.dead_letters().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_target_is_recorded_as_dead_letter() {
    let mut runtime = Runtime::new(16, std::sync::Arc::new(TokioBackend));

    let _counter_addr = runtime
        .register_actor(
            ActorId(1),
            CounterActor::new(CounterModel::default()),
            CounterActor::effect_driver(Default::default()),
        )
        .expect("counter id must be unique");

    runtime
        .send_envelope(Envelope::new(ActorId(999), CounterMsg::Tick))
        .await
        .expect("mailbox should be open");

    let exit = runtime.run_until_idle().await;
    assert_eq!(exit, RunUntilExit::Idle);

    assert!(
        runtime
            .dead_letters()
            .iter()
            .any(|d| d.reason == DeadLetterReason::UnknownTarget)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn type_mismatch_is_recorded_as_dead_letter() {
    let mut runtime = Runtime::new(16, std::sync::Arc::new(TokioBackend));

    let _counter_addr = runtime
        .register_actor(
            ActorId(1),
            CounterActor::new(CounterModel::default()),
            <CounterActor as Actor>::effect_driver(Default::default()),
        )
        .expect("counter id must be unique");

    runtime
        .send_envelope(Envelope::new(ActorId(1), String::from("not-a-counter-msg")))
        .await
        .expect("mailbox should be open");

    let exit = runtime.run_until_idle().await;
    assert_eq!(exit, RunUntilExit::Idle);

    assert!(
        runtime
            .dead_letters()
            .iter()
            .any(|d| d.reason == DeadLetterReason::TypeMismatch)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn type_lookup_and_weak_refs_are_available_for_singletons() {
    let store = InMemoryStore::default();
    let mut runtime = Runtime::new(64, std::sync::Arc::new(TokioBackend));

    let direct_ref = runtime
        .register_actor(
            ActorId(77),
            CounterActor::new(CounterModel::default()),
            CounterActor::effect_driver(store.clone()),
        )
        .expect("counter id must be unique");

    let by_type = runtime
        .actor_ref::<CounterActor>()
        .expect("type-based lookup should resolve");
    assert_eq!(by_type.actor_id(), direct_ref.actor_id());

    let weak = runtime
        .weak_actor_ref::<CounterActor>()
        .expect("weak type-based lookup should resolve");
    let upgraded = weak.upgrade().expect("runtime still holds sender");
    assert_eq!(upgraded.actor_id(), direct_ref.actor_id());

    by_type
        .send(CounterMsg::IncrementRequested)
        .await
        .expect("mailbox should be open");
    let exit = runtime.run_until_idle().await;
    assert_eq!(exit, RunUntilExit::Idle);

    let model = runtime
        .actor::<CounterActor>(ActorId(77))
        .expect("counter should be registered")
        .lock()
        .await
        .model()
        .clone();
    assert_eq!(model.count, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_enforces_one_actor_instance_per_type() {
    let mut runtime = Runtime::new(16, std::sync::Arc::new(TokioBackend));

    runtime
        .register_actor(
            ActorId(1),
            CounterActor::new(CounterModel::default()),
            CounterActor::effect_driver(Default::default()),
        )
        .expect("first counter registration should succeed");

    let err = runtime
        .register_actor(
            ActorId(2),
            CounterActor::new(CounterModel::default()),
            CounterActor::effect_driver(Default::default()),
        )
        .err()
        .expect("second counter registration should fail for singleton-per-type");

    assert_eq!(
        err,
        RegisterError::DuplicateActorType(std::any::type_name::<CounterActor>())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actor_ref_supports_tell_and_ask_patterns() {
    let mut runtime = Runtime::new(32, std::sync::Arc::new(TokioBackend));
    let adder = runtime
        .register_actor(
            ActorId(900),
            AdderActor::new(AdderModel::default()),
            AdderActor::effect_driver(()),
        )
        .expect("adder registration should succeed");

    adder
        .tell(AdderMsg::Add(7))
        .await
        .expect("tell should enqueue message");
    let exit = runtime.run_until_idle().await;
    assert_eq!(exit, RunUntilExit::Idle);

    let ask_task = tokio::spawn({
        let adder = adder.clone();
        async move { adder.ask(|reply_to| AdderMsg::GetTotal(reply_to)).await }
    });
    let exit = runtime.run_until_predicate(|| ask_task.is_finished()).await;
    assert_eq!(exit, RunUntilExit::ConditionMet);

    let total = ask_task
        .await
        .expect("ask task should not panic")
        .expect("ask should resolve");
    assert_eq!(total, 7);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ask_reports_when_reply_channel_is_dropped() {
    let mut runtime = Runtime::new(32, std::sync::Arc::new(TokioBackend));
    let adder = runtime
        .register_actor(
            ActorId(901),
            AdderActor::new(AdderModel::default()),
            AdderActor::effect_driver(()),
        )
        .expect("adder registration should succeed");

    let ask_task = tokio::spawn({
        let adder = adder.clone();
        async move {
            adder
                .ask(|reply_to| AdderMsg::DropTotalRequest(reply_to))
                .await
        }
    });
    let exit = runtime.run_until_predicate(|| ask_task.is_finished()).await;
    assert_eq!(exit, RunUntilExit::ConditionMet);

    let err = ask_task
        .await
        .expect("ask task should not panic")
        .expect_err("ask should fail when actor drops reply");
    assert_eq!(err, AskError::ResponseChannelClosed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_ref_supports_type_based_tell_and_ask_patterns() {
    let mut runtime = Runtime::new(32, std::sync::Arc::new(TokioBackend));
    runtime
        .register_actor(
            ActorId(902),
            AdderActor::new(AdderModel::default()),
            AdderActor::effect_driver(()),
        )
        .expect("adder registration should succeed");

    let runtime_ref = runtime.runtime_ref();

    runtime_ref
        .tell::<AdderActor>(AdderMsg::Add(11))
        .await
        .expect("type-based tell should resolve");
    let exit = runtime.run_until_idle().await;
    assert_eq!(exit, RunUntilExit::Idle);

    let ask_task = tokio::spawn({
        let runtime_ref = runtime_ref.clone();
        async move {
            runtime_ref
                .ask::<AdderActor, u64, _>(|reply_to| AdderMsg::GetTotal(reply_to))
                .await
        }
    });
    let exit = runtime.run_until_predicate(|| ask_task.is_finished()).await;
    assert_eq!(exit, RunUntilExit::ConditionMet);

    let total = ask_task
        .await
        .expect("ask task should not panic")
        .expect("type-based ask should resolve");
    assert_eq!(total, 11);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_ref_reports_missing_actor_type_for_tell_and_ask() {
    let runtime = Runtime::new(8, std::sync::Arc::new(TokioBackend));
    let runtime_ref = runtime.runtime_ref();

    let tell_err = runtime_ref
        .tell::<AdderActor>(AdderMsg::Add(1))
        .await
        .expect_err("tell should fail when actor type is not registered");
    assert_eq!(
        tell_err,
        RuntimeTellError::ActorTypeNotRegistered(std::any::type_name::<AdderActor>())
    );

    let ask_err = runtime_ref
        .ask::<AdderActor, u64, _>(|reply_to| AdderMsg::GetTotal(reply_to))
        .await
        .expect_err("ask should fail when actor type is not registered");
    assert_eq!(
        ask_err,
        RuntimeAskError::ActorTypeNotRegistered(std::any::type_name::<AdderActor>())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_until_supports_deadline_condition() {
    let mut runtime = Runtime::new(8, std::sync::Arc::new(TokioBackend));
    let exit = runtime
        .run_until(RunUntil::for_duration(Duration::from_millis(1)))
        .await;
    assert_eq!(exit, RunUntilExit::DeadlineReached);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn port_ref_can_target_real_provider_actor() {
    let mut runtime = Runtime::new(32, std::sync::Arc::new(TokioBackend));
    runtime
        .register_actor(
            ActorId(1000),
            RealAccumulatorActor::new(),
            RealAccumulatorActor::effect_driver(()),
        )
        .expect("real provider registration should succeed");

    let port = runtime
        .register_port::<AccumulatorPort, RealAccumulatorActor>()
        .expect("port registration should succeed");

    port.tell(AccumulatorReq::Add(7))
        .await
        .expect("tell through port should succeed");
    let exit = runtime.run_until_idle().await;
    assert_eq!(exit, RunUntilExit::Idle);

    let ask_task = tokio::spawn({
        let port = port.clone();
        async move { port.ask(AccumulatorReq::GetTotal).await }
    });
    let exit = runtime.run_until_predicate(|| ask_task.is_finished()).await;
    assert_eq!(exit, RunUntilExit::ConditionMet);

    let total = ask_task
        .await
        .expect("ask task should not panic")
        .expect("ask should resolve");
    assert_eq!(total, AccumulatorRes::Total(7));
    assert!(
        runtime.dead_letters().is_empty(),
        "port tell replies should be discarded without runtime failures"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn port_ref_can_target_mock_provider_actor() {
    let mut runtime = Runtime::new(32, std::sync::Arc::new(TokioBackend));
    runtime
        .register_actor(
            ActorId(1001),
            MockAccumulatorActor::new(999),
            MockAccumulatorActor::effect_driver(()),
        )
        .expect("mock provider registration should succeed");

    let port = runtime
        .register_port::<AccumulatorPort, MockAccumulatorActor>()
        .expect("port registration should succeed");

    port.tell(AccumulatorReq::Add(7))
        .await
        .expect("tell through port should succeed");
    let exit = runtime.run_until_idle().await;
    assert_eq!(exit, RunUntilExit::Idle);

    let ask_task = tokio::spawn({
        let port = port.clone();
        async move { port.ask(AccumulatorReq::GetTotal).await }
    });
    let exit = runtime.run_until_predicate(|| ask_task.is_finished()).await;
    assert_eq!(exit, RunUntilExit::ConditionMet);

    let total = ask_task
        .await
        .expect("ask task should not panic")
        .expect("ask should resolve");
    assert_eq!(total, AccumulatorRes::Total(999));
    assert!(
        runtime.dead_letters().is_empty(),
        "port tell replies should be discarded without runtime failures"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn register_port_requires_provider_actor_and_unique_port_type() {
    let mut runtime = Runtime::new(32, std::sync::Arc::new(TokioBackend));

    let missing_provider = match runtime.register_port::<AccumulatorPort, RealAccumulatorActor>() {
        Ok(_) => panic!("provider actor type should be required before port binding"),
        Err(err) => err,
    };
    assert_eq!(
        missing_provider,
        RegisterPortError::ProviderActorTypeNotRegistered(std::any::type_name::<
            RealAccumulatorActor,
        >())
    );

    runtime
        .register_actor(
            ActorId(1002),
            RealAccumulatorActor::new(),
            RealAccumulatorActor::effect_driver(()),
        )
        .expect("real provider registration should succeed");
    runtime
        .register_port::<AccumulatorPort, RealAccumulatorActor>()
        .expect("first port registration should succeed");

    let duplicate = match runtime.register_port::<AccumulatorPort, RealAccumulatorActor>() {
        Ok(_) => panic!("duplicate port registrations should fail"),
        Err(err) => err,
    };
    assert_eq!(
        duplicate,
        RegisterPortError::DuplicatePortType(std::any::type_name::<AccumulatorPort>())
    );
}
