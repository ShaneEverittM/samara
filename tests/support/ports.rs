use samara::{
    effects::EffectRun,
    runtime::{
        Actor, EffectContext, EffectDriver, IssuedCmd, Port, PortHandler, ReplyToken, UpdateContext,
    },
};

pub struct AccumulatorPort;

impl Port for AccumulatorPort {
    type Req = AccumulatorReq;
    type Res = AccumulatorRes;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccumulatorReq {
    Add(u64),
    GetTotal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccumulatorRes {
    Ack,
    Total(u64),
}

pub enum PortCmd {
    Reply {
        response: AccumulatorRes,
        reply_to: ReplyToken<AccumulatorRes>,
    },
}

#[derive(Clone, Copy, Default)]
pub struct PortEffectDriver;

impl EffectDriver<PortCmd> for PortEffectDriver {
    fn run(&self, issued: IssuedCmd<PortCmd>, ctx: &EffectContext) -> EffectRun {
        match issued.cmd {
            PortCmd::Reply { response, reply_to } => {
                let runtime = ctx.runtime().clone();
                EffectRun::side_effect_future(async move {
                    let _ = runtime.reply(reply_to, response);
                })
            }
        }
    }
}

pub struct RealAccumulatorActor {
    total: u64,
}

impl RealAccumulatorActor {
    pub fn new() -> Self {
        Self { total: 0 }
    }
}

pub enum RealAccumulatorMsg {
    Request(AccumulatorReq),
}

impl Actor for RealAccumulatorActor {
    type Msg = RealAccumulatorMsg;
    type Cmd = PortCmd;
    type Driver = PortEffectDriver;
    type DriverContext = ();

    fn update(&mut self, msg: Self::Msg, ctx: &UpdateContext) -> Vec<Self::Cmd> {
        match msg {
            RealAccumulatorMsg::Request(req) => {
                let Some(reply_to) = ctx.reply_token::<AccumulatorRes>() else {
                    return Vec::new();
                };
                let response = match req {
                    AccumulatorReq::Add(value) => {
                        self.total += value;
                        AccumulatorRes::Ack
                    }
                    AccumulatorReq::GetTotal => AccumulatorRes::Total(self.total),
                };
                vec![PortCmd::Reply { response, reply_to }]
            }
        }
    }

    fn effect_driver(_context: Self::DriverContext) -> Self::Driver
    where
        Self: Sized,
    {
        PortEffectDriver
    }
}

impl PortHandler<AccumulatorPort> for RealAccumulatorActor {
    fn request(req: AccumulatorReq) -> Self::Msg {
        RealAccumulatorMsg::Request(req)
    }
}

pub struct MockAccumulatorActor {
    fixed_total: u64,
}

impl MockAccumulatorActor {
    pub fn new(fixed_total: u64) -> Self {
        Self { fixed_total }
    }
}

pub enum MockAccumulatorMsg {
    Request(AccumulatorReq),
}

impl Actor for MockAccumulatorActor {
    type Msg = MockAccumulatorMsg;
    type Cmd = PortCmd;
    type Driver = PortEffectDriver;
    type DriverContext = ();

    fn update(&mut self, msg: Self::Msg, ctx: &UpdateContext) -> Vec<Self::Cmd> {
        match msg {
            MockAccumulatorMsg::Request(req) => {
                let Some(reply_to) = ctx.reply_token::<AccumulatorRes>() else {
                    return Vec::new();
                };
                let response = match req {
                    AccumulatorReq::Add(_) => AccumulatorRes::Ack,
                    AccumulatorReq::GetTotal => AccumulatorRes::Total(self.fixed_total),
                };
                vec![PortCmd::Reply { response, reply_to }]
            }
        }
    }

    fn effect_driver(_context: Self::DriverContext) -> Self::Driver
    where
        Self: Sized,
    {
        PortEffectDriver
    }
}

impl PortHandler<AccumulatorPort> for MockAccumulatorActor {
    fn request(req: AccumulatorReq) -> Self::Msg {
        MockAccumulatorMsg::Request(req)
    }
}
