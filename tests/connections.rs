//! Connections are walked at registration, checked at build, and listed in
//! the manifest; configuration is plain data with a default (ADR-0031).

use samara::prelude::*;

pub enum LedgerMessage {
    Add(u64),
}

struct Ledger;

impl Component for Ledger {
    type Io = ();
    type Connections = ();
    type Configuration = ();
    type Model = u64;
    type Message = LedgerMessage;

    fn init(_: &Context<Self>) -> Init<u64, LedgerMessage> {
        Init::new(0)
    }

    fn update(
        _: &Context<Self>,
        total: &mut u64,
        LedgerMessage::Add(amount): LedgerMessage,
    ) -> Command<LedgerMessage> {
        *total += amount;
        Command::none()
    }
}

/// Sends its configured amount to whatever it is connected to.
struct Clerk<W>(std::marker::PhantomData<fn() -> W>);

trait Wiring: Connections {
    fn add(&self, amount: u64) -> Command<()>;
}

impl<W: Wiring> Component for Clerk<W> {
    type Io = ();
    type Connections = W;
    type Configuration = u64;
    type Model = ();
    type Message = ();

    fn init(_: &Context<Self>) -> Init<(), ()> {
        Init::default()
    }

    fn update(cx: &Context<Self>, _: &mut (), _: ()) -> Command<()> {
        cx.to.add(cx.config)
    }
}

impl Wiring for Connector<LedgerMessage> {
    fn add(&self, amount: u64) -> Command<()> {
        Command::send(self, LedgerMessage::Add(amount))
    }
}
impl Wiring for Port<LedgerMessage> {
    fn add(&self, amount: u64) -> Command<()> {
        Command::send(self, LedgerMessage::Add(amount))
    }
}
impl Wiring for ComponentRef<Ledger> {
    fn add(&self, amount: u64) -> Command<()> {
        Command::send(self, LedgerMessage::Add(amount))
    }
}

#[derive(Connections)]
struct Named {
    ledger: Connector<LedgerMessage>,
    audit: Option<Port<LedgerMessage>>,
    mirrors: Vec<ComponentRef<Ledger>>,
}
impl Wiring for Named {
    fn add(&self, amount: u64) -> Command<()> {
        Command::send(&self.ledger, LedgerMessage::Add(amount))
    }
}

fn ledger(program: &mut ProgramBuilder, name: &str) -> InstalledComponent<Ledger> {
    program.configure_component::<Ledger>().name(name).add(())
}

fn clerk<W: Wiring>(program: &mut ProgramBuilder, name: &str, wiring: W) -> ComponentRef<Clerk<W>> {
    program
        .configure_component::<Clerk<W>>()
        .name(name)
        .add(wiring)
        .into()
}

#[test]
fn manifest_lists_each_kind_of_connection_with_its_target() {
    let mut program = Program::builder();
    let main = ledger(&mut program, "main");
    let spare = ledger(&mut program, "spare");
    let audit = program.add_port::<LedgerMessage>(PortId::new("audit"));
    program.connect(&audit, &spare.connector());

    clerk(&mut program, "by-connector", main.connector());
    clerk(&mut program, "by-reference", main.reference().clone());
    clerk(&mut program, "by-port", audit.clone());
    clerk(
        &mut program,
        "by-name",
        Named {
            ledger: main.connector(),
            audit: Some(audit),
            mirrors: vec![main.reference().clone(), spare.reference().clone()],
        },
    );
    clerk(
        &mut program,
        "sparse",
        Named {
            ledger: spare.connector(),
            audit: None,
            mirrors: Vec::new(),
        },
    );

    let manifest = program.build().expect("program assembles").manifest();

    assert!(manifest.capabilities().is_empty());
    assert_eq!(
        manifest.to_string(),
        "by-connector: connection connections::LedgerMessage -> main\n\
         by-reference: connection connections::LedgerMessage -> main\n\
         by-port: connection connections::LedgerMessage -> spare (port PortId(\"audit\"))\n\
         by-name.ledger: connection connections::LedgerMessage -> main\n\
         by-name.audit: connection connections::LedgerMessage -> spare (port PortId(\"audit\"))\n\
         by-name.mirrors: connection connections::LedgerMessage -> main\n\
         by-name.mirrors: connection connections::LedgerMessage -> spare\n\
         sparse.ledger: connection connections::LedgerMessage -> spare\n"
    );
    assert_eq!(
        manifest.connections()[4],
        DeclaredConnection {
            owner: "by-name".into(),
            name: Some("audit".into()),
            message: "connections::LedgerMessage",
            target: "spare".into(),
            port: Some(PortId::new("audit")),
        }
    );
}

#[test]
fn a_connection_from_another_builder_fails_the_build() {
    let mut other = Program::builder();
    let foreign = ledger(&mut other, "foreign");

    let mut program = Program::builder();
    clerk(&mut program, "clerk", foreign.connector());
    let error = program
        .build()
        .err()
        .expect("a foreign connection is rejected");
    assert!(matches!(
        &error,
        ProgramBuildError::ForeignConnection { component, connection: None }
            if component.name() == "clerk"
    ));
    assert_eq!(
        error.to_string(),
        "a connection of clerk belongs to another ProgramBuilder"
    );

    let mut program = Program::builder();
    let own = ledger(&mut program, "own");
    clerk(
        &mut program,
        "clerk",
        Named {
            ledger: own.connector(),
            audit: None,
            mirrors: vec![foreign.reference().clone()],
        },
    );
    let error = program
        .build()
        .err()
        .expect("a foreign connection is rejected");
    assert_eq!(
        error.to_string(),
        "connection clerk.mirrors belongs to another ProgramBuilder"
    );
}

#[test]
fn configuration_defaults_and_the_builder_overrides_it() {
    let mut program = Program::builder();
    let main = ledger(&mut program, "main");
    let defaulted = program.add_component::<Clerk<Connector<LedgerMessage>>>(main.connector());
    let configured = program
        .configure_component::<Clerk<Connector<LedgerMessage>>>()
        .configuration(7)
        .add(main.connector());
    let mut runtime = ControlledRuntime::builder(program.build().unwrap())
        .build()
        .unwrap();

    runtime.send(&defaulted, ()).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(runtime.state(&main).unwrap(), &0);

    runtime.send(&configured, ()).unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(runtime.state(&main).unwrap(), &7);
}

#[test]
fn a_detached_context_is_enough_to_call_update() {
    let mut program = Program::builder();
    let main = ledger(&mut program, "main");
    let cx = Context::<Clerk<Connector<LedgerMessage>>>::detached(main.connector(), 3);

    let command = Clerk::update(&cx, &mut (), ());

    assert!(!command.is_none());
}
