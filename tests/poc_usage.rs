use std::time::Duration;

mod support;

use samara::{
    runtime::{Actor, ActorId, DeadLetterReason, Envelope, Runtime},
    system_effects::TokioBackend,
};
use support::{
    counter::{CounterActor, CounterModel, CounterMsg, self},
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
