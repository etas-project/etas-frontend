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
    assert_eq!(known.runtime_types.len(), 14);
    for (role, ty) in &known.runtime_types {
        assert!(checked.type_store.get(*ty).is_some());
        assert_eq!(known.runtime_role(&checked.type_store, *ty), Some(*role));
    }
    assert!(known.runtime_type(Role::CommandResult).is_some());
    assert!(known.runtime_type(Role::JsonValue).is_some());
}
