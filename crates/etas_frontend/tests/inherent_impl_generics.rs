use etas_frontend::{Frontend, ProjectInput, SourceInput};

#[test]
fn generic_spec_impl_instantiates_owner_through_a_bound_method_witness() {
    let output = Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(
        include_str!("fixtures/method_resolution/generic_spec_owner.es"),
    )));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    assert_eq!(summary.requested_actions.effects.iter().count(), 1);
    assert_eq!(summary.escaping_effects, summary.requested_actions);
    let etas_effects::ActionTraceDomain::Event(event) = &summary.action_trace else {
        panic!(
            "one call of the concrete implementation: {:?}",
            summary.action_trace
        );
    };
    let etas_effects::Effect::AppliedAction(action) = &event.action else {
        panic!("typed action");
    };
    let [etas_types::EffectArgRef::Type(ty)] = action.args.as_slice() else {
        panic!("typed selector");
    };
    assert_eq!(
        checked.type_store.get(*ty),
        Some(&etas_types::Type::Primitive(etas_types::PrimitiveType::I32))
    );
}

#[test]
fn source_impl_owner_binder_is_independent_and_method_binder_can_shadow_it() {
    let output = Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app.main;
type Container<T> = { value: T };
impl Container<T> {
    public flow get(self) -> T ![] { return self.value; }
    public flow replace<T>(self, value: T) -> Container<T> ![] {
        return Container<T> { value };
    }
}

flow main() -> string ![] {
    return Container<i32> { value = 7 }.replace("checked").get();
}
"#,
    )));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let methods = checked
        .types
        .methods
        .entries()
        .filter(|entry| {
            matches!(
                &entry.target,
                etas_types::method::MethodTarget::Source { .. }
            )
        })
        .collect::<Vec<_>>();
    let replace = methods
        .iter()
        .find(|method| method.name == "replace")
        .unwrap();
    assert_eq!(replace.owner_params.len(), 1);
    assert_eq!(replace.signature.generic_params.len(), 1);
    assert_ne!(
        replace.owner_params[0].subject,
        replace.signature.generic_params[0].subject
    );
    for fact in checked.types.checked_method_calls.values() {
        if matches!(
            &fact.target,
            etas_types::method::MethodTarget::Source { .. }
        ) {
            assert_eq!(fact.owner_bindings.len(), 1);
        }
    }
}

#[test]
fn source_impl_owner_inherits_the_declared_constructor_bounds() {
    let output = Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app.main;
spec Allowed;
type Value = { number: i32 };
impl Value ~ Allowed;
type Container<T ~ Allowed> = { value: T };
impl Container<T> {
    public flow get(self) -> T ![] { return self.value; }
}
flow main() -> i32 ![] {
    return Container<Value> { value = Value { number = 7 } }.get().number;
}
"#,
    )));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let get = checked
        .types
        .methods
        .entries()
        .find(|method| method.name == "get")
        .unwrap();
    assert_eq!(get.owner_params[0].bounds.len(), 1);
}
