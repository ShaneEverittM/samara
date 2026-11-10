//! Defense-in-depth checks for the erased effect registries.
use super::*;
use crate::{Program, controlled_runtime::ControlledBindings, live_runtime::LiveBindings};
use std::any::TypeId;

fn bind<D: EffectDescriptor>(
    live: &mut LiveBindings,
    controlled: &mut ControlledBindings,
    token: &CapabilityToken,
) where
    FileDriver: EffectDriver<D>,
{
    live.bind_exact_effect::<D, _>(token, FileDriver::new("unused.log"));
    controlled
        .exact_effects
        .push((token.clone(), TypeId::of::<D>()));
}

#[test]
fn all_declared_operations_need_exact_bindings() {
    let (builder, file) = Program::builder::<File<ReadWrite>>();
    let program = builder.build().unwrap().erase();
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
    let (builder, file) = Program::builder::<File<ReadOnly>>();
    let program = builder.build().unwrap().erase();
    let (_, foreign) = Program::builder::<File<ReadOnly>>();
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
fn type_wide_bindings_cannot_supply_or_override_a_file_resource() {
    let (builder, file) = Program::builder::<File<ReadOnly>>();
    let program = builder.build().unwrap().erase();
    for exact_position in [None, Some(0), Some(1)] {
        let mut live = LiveBindings::new();
        let mut controlled = ControlledBindings::new();
        if exact_position == Some(0) {
            bind::<FileRead>(&mut live, &mut controlled, &file.token);
        }
        live.bind_effect::<FileRead, _>(FileDriver::new("wrong.log"));
        controlled.effects.insert(TypeId::of::<FileRead>());
        if exact_position == Some(1) {
            bind::<FileRead>(&mut live, &mut controlled, &file.token);
        }
        assert!(live.validate(&program).is_err(), "{exact_position:?}");
        assert!(controlled.validate(&program).is_err(), "{exact_position:?}");
    }
}

#[test]
fn distinct_permission_sets_bind_only_their_declared_operations() {
    let (builder, (read, append)) = Program::builder::<(File<ReadOnly>, File<AppendOnly>)>();
    let program = builder.build().unwrap().erase();
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
    let (builder, file) = Program::builder::<File<ReadAppend>>();
    let program = builder.build().unwrap().erase();
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
