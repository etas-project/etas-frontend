use etas_frontend::{Frontend, SourceInput};
use etas_std::RuntimeTypeRole as Role;

#[test]
fn checked_native_roles_are_preserved_by_frontend_materialization() {
    let output = Frontend.check(SourceInput::anonymous("flow main() -> unit { return; }"));
    let checked = output
        .checked
        .as_ref()
        .unwrap_or_else(|| panic!("{:?}", output.diagnostics));
    let known = &checked.types.known_std_types;
    assert_eq!(known.runtime_types.len(), 19);
    for (role, ty) in &known.runtime_types {
        assert!(checked.type_store.get(*ty).is_some());
        assert_eq!(known.runtime_role(&checked.type_store, *ty), Some(*role));
    }
    assert!(known.runtime_type(Role::CommandResult).is_some());
    assert!(known.runtime_type(Role::JsonValue).is_some());
}

#[test]
fn referenced_native_constants_keep_checked_owner_facts_after_materialization() {
    let output = Frontend.check(SourceInput::anonymous(
        r#"
import std.host.sandbox.{SandboxProfile, DefaultCommandSandbox as Profile};
import std.runtime.trace.{TraceLabel, Logical};
flow main() -> (SandboxProfile, TraceLabel) { return (Profile, Logical); }
"#,
    ));
    let checked = output
        .checked
        .as_ref()
        .unwrap_or_else(|| panic!("{:?}", output.diagnostics));
    let known = &checked.types.known_std_types;
    assert_eq!(known.runtime_values.len(), 2);
    for ty in known.runtime_values.values() {
        assert!(checked.type_store.get(*ty).is_some());
        assert!(known.opaque_values.contains(ty));
    }
    let etas_types::ItemSignature::Flow(signature) =
        &checked.types.item_signatures[&checked.entry.unwrap()]
    else {
        panic!("entry signature");
    };
    let etas_types::Type::Tuple(fields) = checked.type_store.get(signature.output).unwrap() else {
        panic!("entry tuple");
    };
    assert_eq!(
        fields.as_slice(),
        &[
            known
                .runtime_value(etas_std::StdValueRole::DefaultCommandSandbox)
                .unwrap(),
            known
                .runtime_value(etas_std::StdValueRole::LogicalTraceLabel)
                .unwrap(),
        ]
    );
}
