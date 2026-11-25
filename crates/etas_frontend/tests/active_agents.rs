use etas_frontend::{Frontend, ProjectInput, SourceInput};

const COUNTER: &str = r#"
module app.counter;
agent Counter = { value: i32 };
impl Counter {
    public flow add(var self, delta: i32) -> i32 ![] {
        self.increase(delta);
        return self.get();
    }
    flow increase(var self, delta: i32) -> unit ![] {
        self.value = self.value + delta;
    }
    public flow get(self) -> i32 ![] {
        return self.value;
    }
}
flow main(args: Array<string>) -> i32 ![] {
    let counter = Counter { value= 0 };
    let alias = counter;
    let read = alias.get;
    let update = alias.add;
    update(1);
    return read();
}
"#;

#[test]
fn active_state_and_receiver_methods_have_checked_identity() {
    let output =
        Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(COUNTER)));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let signature = checked
        .types
        .symbol_types
        .values()
        .find_map(|fact| match fact {
            etas_types::SymbolTypeFact::Agent { signature } => Some(signature),
            _ => None,
        })
        .unwrap();
    assert!(
        matches!(checked.type_store.get(signature.component), Some(etas_types::Type::Nominal(nominal)) if nominal.representation.is_none())
    );
    assert!(
        matches!(checked.type_store.get(signature.state.unwrap()), Some(etas_types::Type::Record(record)) if record.fields.len() == 1 && record.fields[0].name == "value")
    );
    assert!(signature.default_entry.is_none());
    let methods = checked.types.methods.entries().filter(|method| matches!(method.owner, etas_types::method::MethodOwner::Agent(owner) if owner == signature.component)).collect::<Vec<_>>();
    assert_eq!(methods.len(), 3);
    assert!(
        methods
            .iter()
            .any(|method| method.name == "add" && method.receiver.is_updating())
    );
    assert!(
        methods
            .iter()
            .any(|method| method.name == "get" && method.receiver.value_slot() == Some(0))
    );
    assert_eq!(checked.types.bound_method_values.len(), 2);
    assert_eq!(checked.types.agent_methods.len(), 3);
    assert_eq!(
        checked
            .types
            .checked_method_calls
            .values()
            .filter(|call| call.kind == etas_types::facts::methods::MethodCallKind::AgentTurnHelper)
            .count(),
        2
    );
    for bound in checked.types.bound_method_values.values() {
        assert_eq!(
            bound.method.kind,
            etas_types::facts::methods::MethodCallKind::AgentRequest
        );
        assert!(bound.method.request_context.is_some());
    }
    let summary = checked
        .effects
        .item_effects
        .get(&checked.entry.unwrap())
        .unwrap();
    assert_eq!(
        summary.determinism,
        etas_effects::Determinism::NonDeterministic
    );
    let managed = summary
        .requested_actions
        .effects
        .iter()
        .filter_map(|effect| match effect {
            etas_effects::Effect::AppliedAction(action) => checked
                .effect_registry
                .action_name(action.action.tag, action.action.action),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(managed.contains(&"create"));
    assert!(managed.contains(&"call"));
}

#[test]
fn agent_state_is_private_outside_its_receiver() {
    let source =
        format!("{COUNTER}\nflow inspect(counter: Counter) -> i32 ![] {{ return counter.value; }}");
    let output =
        Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(source)));
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("field")
                && diagnostic.message.contains("value")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn generic_agent_state_uses_the_instantiated_owner() {
    let output = Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app.generic;
agent Box<T> = { value: T };
impl Box<T> {
    public flow get(self) -> T ![] { return self.value; }
}
flow main(args: Array<string>) -> i32 ![] {
    let boxed = Box<i32> { value= 42 };
    return boxed.get();
}
"#,
    )));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}
