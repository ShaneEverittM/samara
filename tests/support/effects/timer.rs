use samara::runtime::{EffectFuture, Envelope, IssuedCmd};

use crate::support::counter::{CounterMsg, TimerCmd};

#[derive(Clone, Copy, Default)]
pub struct TimerEffects;

impl TimerEffects {
    pub fn handle(&self, issued: IssuedCmd<TimerCmd>) -> EffectFuture {
        Box::pin(async move {
            let to = issued.origin;
            match issued.cmd {
                TimerCmd::ScheduleTick(delay) => {
                    tokio::time::sleep(delay).await;
                    let env = Envelope::with_meta(to, CounterMsg::Tick, issued.meta);
                    Ok(vec![env])
                }
            }
        })
    }
}
