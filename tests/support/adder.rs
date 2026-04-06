use samara::{
    effects::EffectRun,
    runtime::{Actor, EffectContext, EffectDriver, IssuedCmd, Request, UpdateContext},
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AdderModel {
    pub total: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Add(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GetTotal;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DropTotalRequest;

pub enum AdderMsg {
    Add(u64),
    GetTotal,
    DropTotalRequest,
}

pub enum AdderCmd {
    ReplyTotal { value: u64 },
}

pub struct AdderActor {
    model: AdderModel,
}

impl AdderActor {
    pub fn new(model: AdderModel) -> Self {
        Self { model }
    }
}

impl Request<AdderActor> for Add {
    type Reply = ();

    fn into_msg(self) -> AdderMsg {
        AdderMsg::Add(self.0)
    }
}

impl Request<AdderActor> for GetTotal {
    type Reply = u64;

    fn into_msg(self) -> AdderMsg {
        AdderMsg::GetTotal
    }
}

impl Request<AdderActor> for DropTotalRequest {
    type Reply = u64;

    fn into_msg(self) -> AdderMsg {
        AdderMsg::DropTotalRequest
    }
}

impl Actor for AdderActor {
    type Msg = AdderMsg;
    type Cmd = AdderCmd;
    type Driver = AdderEffectDriver;
    type DriverContext = ();

    fn update(&mut self, msg: Self::Msg, ctx: &UpdateContext) -> Vec<Self::Cmd> {
        match msg {
            AdderMsg::Add(value) => {
                self.model.total += value;
                Vec::new()
            }
            AdderMsg::GetTotal => {
                if !ctx.has_reply() {
                    return Vec::new();
                }
                ctx.claim_reply();
                vec![AdderCmd::ReplyTotal {
                    value: self.model.total,
                }]
            }
            AdderMsg::DropTotalRequest => Vec::new(),
        }
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
    fn run(&self, issued: IssuedCmd<AdderCmd>, ctx: &EffectContext) -> EffectRun {
        match issued.cmd {
            AdderCmd::ReplyTotal { value } => {
                let effect_ctx = ctx.clone();
                let meta = issued.meta.clone();
                EffectRun::side_effect_future(async move {
                    let _ = effect_ctx.reply_from_meta(&meta, value);
                })
            }
        }
    }
}
