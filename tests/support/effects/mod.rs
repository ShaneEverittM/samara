use std::sync::Arc;

use samara::runtime::{EffectHandler, IssuedCmd};

use crate::support::{
    counter::AppCmd,
    store::InMemoryStore,
};

mod persistence;
mod timer;

pub fn app_effect_handler(store: InMemoryStore) -> EffectHandler<AppCmd> {
    let persistence = persistence::PersistenceEffects::new(store);
    let timer = timer::TimerEffects;

    Arc::new(move |issued: IssuedCmd<AppCmd>| {
        let IssuedCmd { origin, meta, cmd } = issued;
        match cmd {
            AppCmd::Persist(cmd) => persistence.handle(IssuedCmd {
                origin,
                meta,
                cmd,
            }),
            AppCmd::Timer(cmd) => timer.handle(IssuedCmd {
                origin,
                meta,
                cmd,
            }),
        }
    })
}
