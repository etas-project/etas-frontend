use etas_frontend::{Frontend, ProjectInput, SourceInput};

#[test]
fn bound_method_values_keep_generic_receiver_owner_bindings() {
    let source = r#"
type Cell<T> = { value: T };
impl Cell<T> {
    flow read(self) -> T { return self.value; }
}
flow make<T>(value: T) -> () -> T ![] {
    let cell = Cell<T> { value = value };
    return cell.read;
}
flow main() -> i32 {
    let read = make(7);
    return read();
}
"#;
    let output =
        Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(source)));
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let method = checked
        .types
        .type_impl_methods
        .values()
        .find(|method| method.name == "read")
        .unwrap();
    let eligibility = &checked.cpu_parallel.callables[&method.body];
    assert!(eligibility.eligible, "{eligibility:?}");
}

#[test]
fn source_method_values_preserve_receiver_and_checked_generic_instantiation() {
    let source = r#"
type Counter = { number: i32 };
impl Counter {
    flow compute(self, value: i32) -> i32 { return self.number + value; }
    flow echo<T>(self, value: T) -> T { return value; }
}
flow make<T>(seed: T) -> (i32) -> i32 ![] {
    let counter = Counter { number = 7 };
    return counter.compute;
}
flow main() -> i32 {
    let counter = Counter { number = 1 };
    let generic: (i32) -> i32 ![] = counter.echo;
    let method = make("creator generic");
    return method(generic(3));
}
"#;
    let output =
        Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(source)));
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    let checked = output.checked.unwrap();
    assert_eq!(checked.types.bound_method_values.len(), 2);
    let method = checked
        .types
        .bound_method_values
        .values()
        .find(|value| !value.method.instantiation.type_bindings.is_empty())
        .unwrap();
    let etas_types::method::MethodTarget::Source { callable, .. } = method.method.target else {
        unreachable!()
    };
    assert!(
        !checked
            .checked_callable_applications(callable)
            .unwrap()
            .is_empty()
    );
}
