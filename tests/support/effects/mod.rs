use samara::runtime::{EffectDriver, IssuedCmd};

use crate::support::{
    counter::CounterCmd,
    store::InMemoryStore,
};

mod persistence;
mod timer;

pub struct CounterEffectDriver {
    persistence: persistence::PersistenceEffects,
    timer: timer::TimerEffects,
}

impl CounterEffectDriver {
    pub fn new(store: InMemoryStore) -> Self {
        Self {
            persistence: persistence::PersistenceEffects::new(store),
            timer: timer::TimerEffects,
        }
    }
}

impl EffectDriver<CounterCmd> for CounterEffectDriver {
    fn run(&self, issued: IssuedCmd<CounterCmd>) -> samara::system_effects::EffectRun {
        let IssuedCmd { origin, meta, cmd } = issued;
        match cmd {
            CounterCmd::Persist(cmd) => self.persistence.handle(IssuedCmd {
                origin,
                meta,
                cmd,
            }),
            CounterCmd::Timer(cmd) => self.timer.handle(IssuedCmd {
                origin,
                meta,
                cmd,
            }),
        }
    }
}
