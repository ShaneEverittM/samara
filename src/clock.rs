//! First-party wall-clock effect.
//!
//! Provisional. `update` never reads a clock itself; it issues
//! [`ReadSystemTime`] and receives the instant as a Message. A controlled test
//! completes the effect with any instant it likes, which is what makes
//! time-dependent logic testable without waiting.

use std::convert::Infallible;
use std::time::SystemTime;

use crate::{BoxFuture, EffectDescriptor, EffectDriver};

/// Asks the host what time it is.
///
/// Always succeeds with a [`SystemTime`]: its error type has no values, so a
/// finisher can take the success directly. Supply [`SystemClockDriver`]
/// for its capability through [`RuntimeSetup::supply`](crate::RuntimeSetup::supply).
///
/// ```
/// use samara::prelude::*;
/// use std::time::SystemTime;
/// let clock = <Effect<ReadSystemTime>>::detached();
/// let command: Command<SystemTime> =
///     Effect::perform(&clock, ReadSystemTime).producing(|EffectOutcome::Succeeded(now)| now);
/// assert!(command.effect_intent::<ReadSystemTime>().is_some());
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ReadSystemTime;

impl EffectDescriptor for ReadSystemTime {
    type Output = SystemTime;
    type Error = Infallible;
}

/// Live driver that answers [`ReadSystemTime`] from the operating system clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClockDriver;

impl EffectDriver<ReadSystemTime> for SystemClockDriver {
    fn execute(&self, _: ReadSystemTime) -> BoxFuture<Result<SystemTime, Infallible>> {
        Box::pin(async { Ok(SystemTime::now()) })
    }
}
