use std::time::Duration;

mod support;

use samara::runtime::{ActorId, DeadLetterReason, Envelope, Meta, Runtime};
use support::{
    counter::{update, CounterActor, CounterModel, CounterMsg},
    effects::app_effect_handler,
    store::InMemoryStore,
};

#[test]
fn update_is_deterministic() {
    let model = CounterModel::default();
    let msg = CounterMsg::IncrementRequested;

    let first = update(model.clone(), msg.clone());
    let second = update(model, msg);

    assert_eq!(first, second);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_can_drive_counter_actor_with_two_effects() {
    let store = InMemoryStore::default();
    let handler = app_effect_handler(store.clone());
    let mut runtime = Runtime::new(128, handler);

    let counter_id = ActorId(1);
    let counter_addr = runtime
        .register_actor(counter_id, CounterActor::new(CounterModel::default()))
        .expect("counter id must be unique");

    counter_addr
        .send_with_meta(
            CounterMsg::IncrementRequested,
            Meta {
                correlation_id: Some(42),
                causation_id: None,
            },
        )
        .await
        .expect("mailbox should be open");

    runtime.run_for(Duration::from_millis(50)).await;

    let model = runtime
        .actor::<CounterActor>(counter_id)
        .expect("counter should be registered")
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
    let mut runtime = Runtime::new(16, app_effect_handler(Default::default()));

    let _counter_addr = runtime
        .register_actor(ActorId(1), CounterActor::new(CounterModel::default()))
        .expect("counter id must be unique");

    runtime
        .send_envelope(Envelope::new(ActorId(999), CounterMsg::Tick))
        .await
        .expect("mailbox should be open");

    runtime.run_for(Duration::from_millis(5)).await;

    assert!(runtime
        .dead_letters()
        .iter()
        .any(|d| d.reason == DeadLetterReason::UnknownTarget));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn type_mismatch_is_recorded_as_dead_letter() {
    let mut runtime = Runtime::new(16, app_effect_handler(Default::default()));

    let _counter_addr = runtime
        .register_actor(ActorId(1), CounterActor::new(CounterModel::default()))
        .expect("counter id must be unique");

    runtime
        .send_envelope(Envelope::new(ActorId(1), String::from("not-a-counter-msg")))
        .await
        .expect("mailbox should be open");

    runtime.run_for(Duration::from_millis(5)).await;

    assert!(runtime
        .dead_letters()
        .iter()
        .any(|d| d.reason == DeadLetterReason::TypeMismatch));
}
