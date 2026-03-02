use tokio::time::Duration;

use samara::{
    runtime::{Actor, EffectContext, EffectDriver, Envelope, IssuedCmd, UpdateContext},
    system_effects::{EffectRun, Sleep},
};

use crate::support::store::InMemoryStore;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CounterModel {
    pub count: u64,
    pub ticks: u64,
    pub persisted_count: Option<u64>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CounterMsg {
    IncrementRequested,
    Tick,
    Persisted(u64),
    PersistFailed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PersistCmd {
    PersistCount(u64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimerCmd {
    ScheduleTick(Duration),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CounterCmd {
    Persist(PersistCmd),
    Timer(TimerCmd),
}

pub fn update(model: &mut CounterModel, msg: CounterMsg) -> Vec<CounterCmd> {
    match msg {
        CounterMsg::IncrementRequested => {
            model.count += 1;
            vec![
                CounterCmd::Persist(PersistCmd::PersistCount(model.count)),
                CounterCmd::Timer(TimerCmd::ScheduleTick(Duration::from_millis(5))),
            ]
        }
        CounterMsg::Tick => {
            model.ticks += 1;
            vec![]
        }
        CounterMsg::Persisted(value) => {
            model.persisted_count = Some(value);
            vec![]
        }
        CounterMsg::PersistFailed(err) => {
            model.last_error = Some(err);
            vec![]
        }
    }
}

pub struct CounterActor {
    model: CounterModel,
}

impl CounterActor {
    pub fn new(model: CounterModel) -> Self {
        Self { model }
    }

    pub fn model(&self) -> &CounterModel {
        &self.model
    }
}

impl Actor for CounterActor {
    type Msg = CounterMsg;
    type Cmd = CounterCmd;
    type Driver = CounterEffectDriver;
    type DriverContext = InMemoryStore;

    fn update(&mut self, msg: Self::Msg, _ctx: &UpdateContext) -> Vec<Self::Cmd> {
        update(&mut self.model, msg)
    }

    fn effect_driver(context: Self::DriverContext) -> Self::Driver
    where
        Self: Sized,
    {
        CounterEffectDriver::new(context)
    }
}

pub struct CounterEffectDriver {
    persistence: PersistenceEffects,
    timer: TimerEffects,
}

impl CounterEffectDriver {
    pub fn new(store: InMemoryStore) -> Self {
        Self {
            persistence: PersistenceEffects::new(store),
            timer: TimerEffects,
        }
    }
}

impl EffectDriver<CounterCmd> for CounterEffectDriver {
    fn run(&self, issued: IssuedCmd<CounterCmd>, _ctx: &EffectContext) -> EffectRun {
        let IssuedCmd { origin, meta, cmd } = issued;
        match cmd {
            CounterCmd::Persist(cmd) => self.persistence.handle(IssuedCmd { origin, meta, cmd }),
            CounterCmd::Timer(cmd) => self.timer.handle(IssuedCmd { origin, meta, cmd }),
        }
    }
}

#[derive(Clone)]
struct PersistenceEffects {
    store: InMemoryStore,
}

impl PersistenceEffects {
    fn new(store: InMemoryStore) -> Self {
        Self { store }
    }

    fn handle(&self, issued: IssuedCmd<PersistCmd>) -> EffectRun {
        let store = self.store.clone();
        EffectRun::user(async move {
            let to = issued.origin;
            match issued.cmd {
                PersistCmd::PersistCount(value) => {
                    if value == u64::MAX {
                        let env = Envelope::with_meta(
                            to,
                            CounterMsg::PersistFailed("simulated persist failure".to_string()),
                            issued.meta,
                        );
                        Ok(vec![env])
                    } else {
                        store.push(value).await;
                        let env =
                            Envelope::with_meta(to, CounterMsg::Persisted(value), issued.meta);
                        Ok(vec![env])
                    }
                }
            }
        })
    }
}

#[derive(Clone, Copy, Default)]
struct TimerEffects;

impl TimerEffects {
    fn handle(&self, issued: IssuedCmd<TimerCmd>) -> EffectRun {
        let to = issued.origin;
        match issued.cmd {
            TimerCmd::ScheduleTick(delay) => {
                let meta = issued.meta;
                EffectRun::system_effect(Sleep(delay))
                    .on_ok(move |_| vec![Envelope::with_meta(to, CounterMsg::Tick, meta)])
                    .into_run()
            }
        }
    }
}
