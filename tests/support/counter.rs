use tokio::time::Duration;

use samara::runtime::Actor;

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
pub enum AppCmd {
    Persist(PersistCmd),
    Timer(TimerCmd),
}

pub fn update(mut model: CounterModel, msg: CounterMsg) -> (CounterModel, Vec<AppCmd>) {
    match msg {
        CounterMsg::IncrementRequested => {
            model.count += 1;
            let cmds = vec![
                AppCmd::Persist(PersistCmd::PersistCount(model.count)),
                AppCmd::Timer(TimerCmd::ScheduleTick(Duration::from_millis(5))),
            ];
            (model, cmds)
        }
        CounterMsg::Tick => {
            model.ticks += 1;
            (model, vec![])
        }
        CounterMsg::Persisted(value) => {
            model.persisted_count = Some(value);
            (model, vec![])
        }
        CounterMsg::PersistFailed(err) => {
            model.last_error = Some(err);
            (model, vec![])
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
    type Cmd = AppCmd;

    fn on_msg(&mut self, msg: Self::Msg) -> Vec<Self::Cmd> {
        let current = std::mem::take(&mut self.model);
        let (next, cmds) = update(current, msg);
        self.model = next;
        cmds
    }
}
