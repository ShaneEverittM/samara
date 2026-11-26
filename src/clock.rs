//! First-party wall-clock effect.
//!
//! Provisional. `update` never reads a clock itself; it issues
//! [`ReadSystemTime`] and receives the instant as a Message. A controlled test
//! completes the effect with any instant it likes, which is what makes
//! time-dependent logic testable without waiting.

use std::time::SystemTime;

use crate::{BoxFuture, Effect, EffectBuilder, EffectDescriptor, EffectDriver};

/// A Component's capability to read the system clock.
///
/// Always succeeds with a [`SystemTime`]: its error type has no values, so a
/// finisher can take the success directly. Supply [`SystemClockDriver`]
/// for its capability through [`RuntimeSetup::supply`](crate::RuntimeSetup::supply).
///
/// ```
/// use samara::prelude::*;
/// use std::time::SystemTime;
/// let clock = Clock::detached();
/// let command: Command<SystemTime> = Clock::system_time(&clock).on_success(|now| now);
/// assert!(command.effect_intent::<ReadSystemTime>().is_some());
/// ```
pub type Clock = Effect<ReadSystemTime>;

impl Effect<ReadSystemTime> {
    /// Describes one wall-clock read through this capability.
    pub fn system_time(&self) -> EffectBuilder<ReadSystemTime> {
        Effect::perform(self, ReadSystemTime)
    }
}

/// The effect descriptor issued by [`Clock::system_time`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ReadSystemTime;

impl EffectDescriptor for ReadSystemTime {
    type Output = SystemTime;
    type Error = !;
}

/// Live driver that answers [`ReadSystemTime`] from the operating system clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClockDriver;

impl EffectDriver<ReadSystemTime> for SystemClockDriver {
    fn execute(&self, _: ReadSystemTime) -> BoxFuture<Result<SystemTime, !>> {
        Box::pin(async { Ok(SystemTime::now()) })
    }
}
