use tokio::sync::oneshot;

use samara::{
    runtime::{Actor, EffectContext, EffectDriver, IssuedCmd, UpdateContext},
    effects::EffectRun,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AdderModel {
    pub total: u64,
}

pub enum AdderMsg {
    Add(u64),
    GetTotal(oneshot::Sender<u64>),
    DropTotalRequest(oneshot::Sender<u64>),
}

pub enum AdderCmd {
    ReplyTotal {
        value: u64,
        reply_to: oneshot::Sender<u64>,
    },
}

pub fn update(model: &mut AdderModel, msg: AdderMsg) -> Vec<AdderCmd> {
    match msg {
        AdderMsg::Add(value) => {
            model.total += value;
            Vec::new()
        }
        AdderMsg::GetTotal(reply_to) => vec![AdderCmd::ReplyTotal {
            value: model.total,
            reply_to,
        }],
        AdderMsg::DropTotalRequest(reply_to) => {
            drop(reply_to);
            Vec::new()
        }
    }
}

pub struct AdderActor {
    model: AdderModel,
}

impl AdderActor {
    pub fn new(model: AdderModel) -> Self {
        Self { model }
    }
}

impl Actor for AdderActor {
    type Msg = AdderMsg;
    type Cmd = AdderCmd;
    type Driver = AdderEffectDriver;
    type DriverContext = ();

    fn update(&mut self, msg: Self::Msg, _ctx: &UpdateContext) -> Vec<Self::Cmd> {
        update(&mut self.model, msg)
    }

    fn effect_driver(_context: Self::DriverContext) -> Self::Driver
    where
        Self: Sized,
    {
        AdderEffectDriver
    }
}

#[derive(Clone, Copy, Default)]
pub struct AdderEffectDriver;

impl EffectDriver<AdderCmd> for AdderEffectDriver {
    fn run(&self, issued: IssuedCmd<AdderCmd>, _ctx: &EffectContext) -> EffectRun {
        match issued.cmd {
            AdderCmd::ReplyTotal { value, reply_to } => EffectRun::side_effect_future(async move {
                let _ = reply_to.send(value);
            }),
        }
    }
}
