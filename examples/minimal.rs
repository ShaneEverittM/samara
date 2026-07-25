//! The shallowest complete Samara application shape.
//!
//! `Counter` owns one integer and receives increments from an ordinary Tokio
//! `mpsc` channel through Samara's first-party [`StreamDescriptor`] bridge.
//! Its transition is synchronous and side effect free: only a
//! `CounterMessage` can change `CounterModel`, while the runtime owns channel
//! polling and delivery.
//!
//! The `program` function is shared by live execution and the controlled test.
//! Only the runtime binding changes from a real Tokio receiver to scripted
//! input.
//!
//! # Live and controlled profiles
//!
//! End-to-end tests run this exact program with both scripted controlled input
//! and a real Tokio channel. The Component and logical assembly are unchanged;
//! only the boundary binding differs.

use std::error::Error;

use samara::prelude::*;

/// Logical first-party input bound by the surrounding runtime world.
const INCREMENT_INPUT: &str = "counter/increment-input";

/// Component-local identity used to reconcile its desire for that input.
const INCREMENT_SUBSCRIPTION: &str = "increments";

/// All mutable application state owned by the Counter Component.
#[derive(Debug, Default, PartialEq, Eq)]
struct CounterModel {
    /// Sum of every increment delivered by the input source.
    total: u64,

    /// Whether the finite demonstration input has ended normally.
    input_closed: bool,
}

/// The complete set of messages that may change `CounterModel`.
#[derive(Debug)]
enum CounterMessage {
    /// One value arrived through the Tokio channel bridge.
    Increment(u64),

    /// The channel sender was dropped and no more values can arrive.
    InputClosed,
}

/// One Counter instance and its immutable logical input wiring.
///
/// `StreamDescriptor` is only a descriptor. The live `Receiver` remains at assembly
/// time, outside both this value and `CounterModel`.
struct Counter {
    increments: StreamDescriptor<u64>,
}

impl Component for Counter {
    type Model = CounterModel;
    type Message = CounterMessage;

    fn init(&self) -> Init<Self::Model, Self::Message> {
        Init::new(CounterModel::default())
    }

    fn update(&self, model: &mut Self::Model, message: Self::Message) -> Command<Self::Message> {
        match message {
            CounterMessage::Increment(amount) => {
                // This is the whole state transition. It performs no channel
                // read, Tokio call, spawn, or other world interaction.
                model.total += amount;
            }
            CounterMessage::InputClosed => {
                model.input_closed = true;
            }
        }

        // This Component has no finite effects to request.
        Command::none()
    }

    fn subscriptions(&self, model: &Self::Model) -> Subscriptions<Self::Message> {
        if model.input_closed {
            // Once the source reports normal completion, retaining desire for
            // it would ask reconciliation to start the same source again.
            return Subscriptions::none();
        }

        Subscriptions::one(Subscription::source_with(
            SubscriptionId::new(INCREMENT_SUBSCRIPTION),
            self.increments.clone(),
            |event| match event {
                SourceEvent::Item(amount) => CounterMessage::Increment(amount),
                SourceEvent::Ended => CounterMessage::InputClosed,
                SourceEvent::Failed(never) => match never {},
            },
        ))
    }
}

/// Declares the logical application once for every execution profile.
///
/// No Tokio receiver or scripted test input enters this function. It contains
/// only the Component and the logical source descriptor they share.
fn program(increments: StreamDescriptor<u64>) -> (Program, ComponentRef<Counter>) {
    let mut program = Program::builder();
    let counter = program.component(ComponentId::new("counter"), Counter { increments });
    (
        program.build().expect("the example graph is valid"),
        counter,
    )
}

/// Runs the program with an ordinary Tokio channel as its world-facing input.
///
/// In a real runtime, the two sends become `CounterMessage::Increment` messages
/// and dropping `input` becomes `CounterMessage::InputClosed`. Samara owns that
/// bridge; the application does not write a polling task or Driver.
async fn run_live() -> Result<(), Box<dyn Error>> {
    let increments = StreamDescriptor::named(INCREMENT_INPUT);
    let (input, receiver) = tokio::sync::mpsc::channel(1);
    let (program, _counter) = program(increments.clone());

    // Fill the one-slot upstream channel before spawning. This is ordinary
    // Tokio pressure; Samara has not accepted the value yet.
    input.send(2).await?;

    // Binding transfers the unique receiver into Samara's structured runtime
    // scope. The Component still contains only `StreamDescriptor`.
    let runtime = LiveRuntime::builder(program)
        .bind_mpsc(increments, receiver)
        .build()?;
    let runtime = runtime.spawn();

    // With capacity one, each later send can finish only after the bridge has
    // received the prior value. The bridge calls `SourceSink::emit` before its
    // next receive, so the two harmless zero-valued sentinels prove that 2 and
    // 3 crossed Samara's acceptance boundary before shutdown begins.
    input.send(3).await?;
    input.send(0).await?;
    input.send(0).await?;
    drop(input);

    // Drain stops the ongoing Source, but retains the already accepted values
    // and all finite work they causally emit.
    let report = runtime.shutdown(Shutdown::Drain).await?;
    assert!(report.is_clean());
    Ok(())
}

/// Normal Tokio entry point for the canonical onboarding shape.
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    run_live().await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercises the pure transition and declarative subscription immediately.
    #[test]
    fn component_logic_is_directly_testable() {
        let increments = StreamDescriptor::named(INCREMENT_INPUT);
        let counter = Counter {
            increments: increments.clone(),
        };
        let mut model = counter.init().model;

        assert_eq!(
            counter
                .subscriptions(&model)
                .find::<StreamDescriptor<u64>>(&SubscriptionId::new(INCREMENT_SUBSCRIPTION)),
            Some(&increments)
        );

        let command = counter.update(&mut model, CounterMessage::Increment(2));
        assert_eq!(model.total, 2);
        assert!(command.is_none());

        let _ = counter.update(&mut model, CounterMessage::InputClosed);
        assert!(model.input_closed);
        assert_eq!(counter.subscriptions(&model).iter().count(), 0);
    }

    /// Specifies the controlled counterpart of `run_live` using the same
    /// Component and `program` factory.
    ///
    /// This is the canonical whole-program controlled onboarding test. It uses
    /// exactly the same Component and program factory as the live shape.
    #[test]
    fn controlled_stream_updates_the_same_program() -> Result<(), RuntimeError> {
        let increments = StreamDescriptor::named(INCREMENT_INPUT);
        let (program, counter) = program(increments.clone());
        let mut runtime = ControlledRuntime::builder(program)
            .control_stream(increments.clone())
            .build()?;

        runtime.emit_stream(&increments, 2)?;
        runtime.emit_stream(&increments, 3)?;
        runtime.close_stream(&increments)?;
        runtime.run_until_idle()?;

        assert_eq!(
            runtime.state(&counter)?,
            &CounterModel {
                total: 5,
                input_closed: true,
            }
        );

        // Closing the finite source is application-visible input; consuming
        // cancellation then proves the controlled runtime owns nothing else.
        let report = runtime.cancel()?;
        assert!(report.is_clean());
        Ok(())
    }

    /// Runs the canonical onboarding Component through the first-party Tokio
    /// `mpsc` bridge and structured live shutdown.
    #[tokio::test]
    async fn v11_minimal_component_runs_with_live_mpsc() -> Result<(), Box<dyn Error>> {
        run_live().await
    }
}
