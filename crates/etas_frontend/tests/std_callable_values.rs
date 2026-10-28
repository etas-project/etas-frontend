use etas_effects::InterpreterSupport;
use etas_frontend::{Frontend, ProjectInput, ProjectOutput, SourceInput};
use etas_types::Type;

fn check(source: &str) -> ProjectOutput {
    let output =
        Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(source)));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(output.checked.is_some());
    output
}

fn assert_checked_constructor(output: &ProjectOutput) {
    let checked = output.checked.as_ref().unwrap();
    let facts = &checked.effects;
    let constructor = facts
        .std_callable_values
        .values()
        .find(|fact| fact.intrinsic == etas_std::intrinsic::pure::OPTION_SOME)
        .expect("Some reference must have its own checked intrinsic summary");
    assert!(constructor.summary.escaping_effects.is_empty());
    assert!(constructor.summary.requested_actions.is_empty());
    assert_eq!(constructor.summary.support, InterpreterSupport::LocalOnly);
    let Some(Type::Function(signature)) = checked.type_store.get(constructor.flow_type) else {
        panic!("checked constructor signature")
    };
    let row = signature
        .effects
        .as_ref()
        .expect("registry constructors have a known closed row");
    assert!(row.effects.is_empty());
    assert!(row.tail.is_none());
}

#[test]
fn generic_constructor_alias_has_checked_latent_facts() {
    let output = check(
        r#"
module app.main;
type UserId = string;
flow wrap<T>(value: T) -> Option<T> {
    let constructor: (T) -> Option<T> ![] = Some;
    return constructor(value);
}
flow main() -> bool {
    let id = UserId("payload");
    return match wrap(id) { Some(value) => value == id, None => false, };
}
"#,
    );
    assert_checked_constructor(&output);
}

#[test]
fn returned_generic_constructor_materializes_the_closed_callable_type() {
    let output = check(
        r#"
module app.main;
type UserId = string;
flow factory<T>() -> ((T) -> Option<T> ![]) { return Some; }
flow main() -> ((UserId) -> Option<UserId> ![]) { return factory<UserId>(); }
"#,
    );
    assert_checked_constructor(&output);
}

#[test]
fn returned_constructor_specializes_nested_callback_effect_rows() {
    let output = check(
        r#"
module app.main;
flow factory<effect E>() -> ((() -> unit ![E]) -> Option<() -> unit ![E]> ![]) ![] {
    return Some;
}
flow write() -> unit ![Console.stdout_write] { perform Console.stdout_write("payload"); }
flow main() -> unit ![Console.stdout_write] {
    let constructor = factory<![Console.stdout_write]>();
    let optional = constructor(write);
    match optional { Some(callback) => { callback(); }, None => { return; }, }
}
"#,
    );
    assert_checked_constructor(&output);
    assert_stdout_requested(&output);
}

#[test]
fn returned_constructor_does_not_erase_a_callback_effect_row_mismatch() {
    let output = Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app.main;
flow factory<effect E>() -> ((() -> unit ![E]) -> Option<() -> unit ![E]> ![]) ![] {
    return Some;
}
flow write_error() -> unit ![Console.stderr_write] { perform Console.stderr_write("payload"); }
flow main() -> unit {
    let constructor = factory<![Console.stdout_write]>();
    let optional = constructor(write_error);
}
"#,
    )));
    assert!(output.checked.is_none());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == etas_core::DiagnosticCode::Type(etas_core::TypeDiagnosticCode::TypeMismatch)),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn constructor_alias_keeps_the_wrapped_callback_actions() {
    let output = check(
        r#"
module app.main;
alias Job = () -> unit ![Console.stdout_write];
flow write() -> unit ![Console.stdout_write] { perform Console.stdout_write("payload"); }
flow main() -> unit ![Console.stdout_write] {
    let constructor: (Job) -> Option<Job> ![] = Some;
    let optional = constructor(write);
    match optional { Some(callback) => { callback(); }, None => { return; }, }
}
"#,
    );
    assert_checked_constructor(&output);
    assert_stdout_requested(&output);
}

fn assert_stdout_requested(output: &ProjectOutput) {
    let checked = output.checked.as_ref().unwrap();
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    let stdout = etas_effects::Effect::Action(etas_effects::ActionRef {
        tag: etas_effects::CONSOLE_TAG,
        action: etas_effects::CONSOLE_STDOUT_WRITE_ACTION,
    });
    assert!(
        summary.escaping_effects.effects.contains(&stdout),
        "{summary:?}"
    );
    assert!(
        summary.requested_actions.effects.contains(&stdout),
        "{summary:?}"
    );
    assert!(summary.escaping_effects.open.is_none());
    assert!(summary.requested_actions.open.is_none());
}

#[test]
fn a_source_function_named_some_is_not_treated_as_a_pure_constructor() {
    let output = check(
        r#"
module app.main;
flow Some(value: i32) -> i32 ![Console.stdout_write] {
    perform Console.stdout_write("not a builtin");
    return value;
}
flow main() -> i32 ![Console.stdout_write] {
    let constructor = Some;
    return constructor(42);
}
"#,
    );
    assert_stdout_requested(&output);
    assert!(
        !output
            .checked
            .as_ref()
            .unwrap()
            .effects
            .std_callable_values
            .values()
            .any(|fact| fact.intrinsic == etas_std::intrinsic::pure::OPTION_SOME)
    );
}

#[test]
fn a_standard_effectful_flow_alias_keeps_requested_actions() {
    let output = check(
        r#"
module app.main;
import std.io.println;
flow main() -> unit {
    let output = println;
    output("standard flow alias");
}
"#,
    );
    let checked = output.checked.as_ref().unwrap();
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    let stdout = etas_effects::Effect::Action(etas_effects::ActionRef {
        tag: etas_effects::CONSOLE_TAG,
        action: etas_effects::CONSOLE_STDOUT_WRITE_ACTION,
    });
    assert!(
        summary.requested_actions.effects.contains(&stdout),
        "{summary:?}"
    );
}

#[test]
fn trace_spec_cannot_lose_actions_through_a_constructor_alias_and_match() {
    let output = Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(
        r#"
module app.main;
spec NoWrites: trace = -Console.stdout_write;
alias Job = () -> unit ![Console.stdout_write];
flow write() -> unit ![Console.stdout_write] { perform Console.stdout_write("payload"); }
flow main() -> unit ![Console.stdout_write] ~ NoWrites {
    let constructor: (Job) -> Option<Job> ![] = Some;
    let optional = constructor(write);
    match optional { Some(callback) => { callback(); }, None => { return; }, }
}
"#,
    )));
    assert!(output.checked.is_none());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == etas_core::DiagnosticCode::Effect(etas_core::EffectDiagnosticCode::TraceSpecDenied)),
        "{:?}",
        output.diagnostics
    );
    assert!(
        !output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == etas_core::DiagnosticCode::Effect(
                etas_core::EffectDiagnosticCode::IncompleteEffectFacts
            )),
        "{:?}",
        output.diagnostics
    );
}
