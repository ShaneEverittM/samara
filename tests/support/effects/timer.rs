use samara::{
    runtime::{Envelope, IssuedCmd},
    system_effects::{composed, infallible_to_envelopes, EffectRun, Sleep},
};

use crate::support::counter::{CounterMsg, TimerCmd};

#[derive(Clone, Copy, Default)]
pub struct TimerEffects;

impl TimerEffects {
    pub fn handle(&self, issued: IssuedCmd<TimerCmd>) -> EffectRun {
        let to = issued.origin;
        match issued.cmd {
            TimerCmd::ScheduleTick(delay) => {
                let meta = issued.meta;
                let effect = composed(
                    Sleep(delay),
                    move |_| vec![Envelope::with_meta(to, CounterMsg::Tick, meta)],
                    infallible_to_envelopes,
                );
                EffectRun::Composed(vec![Box::new(effect)])
            }
        }
    }
}
