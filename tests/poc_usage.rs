use std::time::Duration;

mod support;

use samara::{
    runtime::{
        Actor, ActorId, AskError, DeadLetterReason, Envelope, RegisterError, Runtime,
        RuntimeAskError, RuntimeTellError,
    },
    system_effects::TokioBackend,
};
use support::{
    adder::{AdderActor, AdderModel, AdderMsg},
    counter::{self, CounterActor, CounterModel, CounterMsg},
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

    runtime.run_for(Duration::from_millis(50)).await;

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

    runtime.run_for(Duration::from_millis(5)).await;

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

    runtime.run_for(Duration::from_millis(5)).await;

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
    runtime.run_for(Duration::from_millis(50)).await;

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
    runtime.run_for(Duration::from_millis(10)).await;

    let ask_task = tokio::spawn({
        let adder = adder.clone();
        async move { adder.ask(|reply_to| AdderMsg::GetTotal(reply_to)).await }
    });
    tokio::task::yield_now().await;
    runtime.run_for(Duration::from_millis(10)).await;

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
    tokio::task::yield_now().await;
    runtime.run_for(Duration::from_millis(10)).await;

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
    runtime.run_for(Duration::from_millis(10)).await;

    let ask_task = tokio::spawn({
        let runtime_ref = runtime_ref.clone();
        async move {
            runtime_ref
                .ask::<AdderActor, u64, _>(|reply_to| AdderMsg::GetTotal(reply_to))
                .await
        }
    });
    tokio::task::yield_now().await;
    runtime.run_for(Duration::from_millis(10)).await;

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
