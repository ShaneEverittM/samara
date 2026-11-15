//! Defense-in-depth checks for the erased effect registries.
use super::*;
use crate::{Program, controlled_runtime::ControlledBindings, live_runtime::LiveBindings};
use std::any::TypeId;
use std::marker::PhantomData;

/// Owns `C` and never uses it: registration is the only way to create a
/// capability inside a Program.
struct Holder<C>(PhantomData<C>);
impl<C: crate::Capabilities> crate::Component for Holder<C> {
    type Config = ();
    type Io = C;
    type Model = ();
    type Message = ();
    fn init(_: &(), _: &C) -> crate::Init<(), ()> {
        crate::Init::default()
    }
    fn update(_: &(), _: &C, _: &mut (), _: ()) -> crate::Command<()> {
        crate::Command::none()
    }
}

/// A Program whose one Component owns `C`, and that `C`.
fn program_owning<C: crate::Capabilities>() -> (Program, C) {
    let mut builder = Program::builder();
    let holder = builder.component::<Holder<C>>(());
    (builder.build().unwrap(), holder.io.clone())
}

fn bind<D: EffectDescriptor>(
    live: &mut LiveBindings,
    controlled: &mut ControlledBindings,
    token: &CapabilityToken,
) where
    FileDriver: EffectDriver<D>,
{
    live.bind_effect::<D, _>(token, FileDriver::new("unused.log"));
    controlled
        .exact_effects
        .push((token.clone(), TypeId::of::<D>()));
}

#[test]
fn all_declared_operations_need_exact_bindings() {
    let (program, file) = program_owning::<File<ReadWrite>>();
    let mut live = LiveBindings::new();
    let mut controlled = ControlledBindings::new();
    bind::<FileRead>(&mut live, &mut controlled, &file.token);
    assert!(live.validate(&program).is_err(), "append is missing");
    assert!(controlled.validate(&program).is_err());
    bind::<FileAppend>(&mut live, &mut controlled, &file.token);
    assert!(live.validate(&program).is_err(), "replacement is missing");
    assert!(controlled.validate(&program).is_err());
    bind::<FileReplace>(&mut live, &mut controlled, &file.token);
    live.validate(&program).unwrap();
    controlled.validate(&program).unwrap();
}

#[test]
fn exact_bindings_reject_duplicates_foreign_tokens_and_undeclared_operations() {
    let (program, file) = program_owning::<File<ReadOnly>>();
    let foreign = <File<ReadOnly> as crate::Capabilities>::detached();
    for invalid in ["duplicate", "foreign", "undeclared"] {
        let mut live = LiveBindings::new();
        let mut controlled = ControlledBindings::new();
        bind::<FileRead>(&mut live, &mut controlled, &file.token);
        match invalid {
            "duplicate" => bind::<FileRead>(&mut live, &mut controlled, &file.token),
            "foreign" => bind::<FileRead>(&mut live, &mut controlled, &foreign.token),
            "undeclared" => bind::<FileAppend>(&mut live, &mut controlled, &file.token),
            _ => unreachable!(),
        }
        assert!(live.validate(&program).is_err(), "{invalid}");
        assert!(controlled.validate(&program).is_err(), "{invalid}");
    }
}

#[test]
fn distinct_permission_sets_bind_only_their_declared_operations() {
    let (program, (read, append)) = program_owning::<(File<ReadOnly>, File<AppendOnly>)>();
    let mut live = LiveBindings::new();
    let mut controlled = ControlledBindings::new();
    bind::<FileRead>(&mut live, &mut controlled, &read.token);
    bind::<FileAppend>(&mut live, &mut controlled, &append.token);
    live.validate(&program).unwrap();
    controlled.validate(&program).unwrap();
    assert!(program.declares_effect(&read.token, TypeId::of::<FileRead>()));
    assert!(!program.declares_effect(&read.token, TypeId::of::<FileAppend>()));
    assert!(!program.declares_effect(&append.token, TypeId::of::<FileRead>()));
    assert!(!program.declares_effect(&read.token, TypeId::of::<FileReplace>()));
    assert!(!program.declares_effect(&append.token, TypeId::of::<FileReplace>()));
}

#[test]
fn read_append_declaration_rejects_a_replacement_binding() {
    let (program, file) = program_owning::<File<ReadAppend>>();
    let mut live = LiveBindings::new();
    let mut controlled = ControlledBindings::new();
    bind::<FileRead>(&mut live, &mut controlled, &file.token);
    bind::<FileAppend>(&mut live, &mut controlled, &file.token);
    live.validate(&program).unwrap();
    controlled.validate(&program).unwrap();
    bind::<FileReplace>(&mut live, &mut controlled, &file.token);
    assert!(
        live.validate(&program)
            .unwrap_err()
            .to_string()
            .contains("no matching resource operation")
    );
    assert!(
        controlled
            .validate(&program)
            .unwrap_err()
            .to_string()
            .contains("no matching resource operation")
    );
}
