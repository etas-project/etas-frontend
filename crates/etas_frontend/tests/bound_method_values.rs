use etas_frontend::{Frontend, ProjectInput, SourceInput};

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
}
