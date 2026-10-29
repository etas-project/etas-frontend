use etas_frontend::{Frontend, ProjectInput, SourceInput};

fn check(source: &str) -> etas_frontend::ProjectOutput {
    Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(source)))
}

#[test]
fn semicolon_discards_generic_callback_output_before_handler_answer_inference() {
    let output = check(
        r#"
module app.main;
flow shield<T>(callback: () -> T ![Console.stdout_write]) -> unit {
    handle { callback(); } with { Console.stdout_write(_) => { finish (); } };
    return;
}
flow main() -> unit {
    shield(() => { perform Console.stdout_write("handled"); return 7; });
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let summary = &checked.effects.item_effects[&checked.entry.unwrap()];
    assert!(summary.escaping_effects.is_empty(), "{summary:?}");
    assert_eq!(
        summary.requested_actions.effects.iter().count(),
        1,
        "{summary:?}"
    );
}

#[test]
fn semicolon_block_and_lambda_results_are_unit() {
    let output = check(
        r#"
module app.main;
flow main() -> unit {
    let value: unit = { 7; };
    let callback: () -> unit = () => { 8; };
    callback();
    return value;
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(output.checked.is_some());
}

#[test]
fn semicolon_cannot_supply_a_non_unit_block_result() {
    let output = check("module main; flow main() -> i32 { return { 7; }; }");
    assert!(output.checked.is_none());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == etas_core::DiagnosticCode::Type(etas_core::TypeDiagnosticCode::TypeMismatch)),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn trailing_expressions_and_nonreturning_statements_keep_their_types() {
    for source in [
        "module main; flow main() -> i32 { return { 7 }; }",
        "module main; flow stop() -> never { return abort(\"stop\"); } flow main() -> i32 { return { stop(); }; }",
        "module main; flow main() -> i32 { return { return 7; }; }",
    ] {
        let output = check(source);
        assert!(
            output.diagnostics.is_empty(),
            "{source}: {:?}",
            output.diagnostics
        );
        assert!(output.checked.is_some(), "{source}");
    }
}

#[test]
fn nonreturning_paths_do_not_preempt_inhabited_lambda_return_candidates() {
    for body in ["return 7;", "if true { return 7; } abort(\"unreachable\");"] {
        let output = check(&format!(
            "module main; flow apply<T>(callback: () -> T) -> T {{ return callback(); }} flow main() -> i32 {{ return apply(() => {{ {body} }}); }}"
        ));
        assert!(
            output.diagnostics.is_empty(),
            "{body}: {:?}",
            output.diagnostics
        );
        assert!(output.checked.is_some(), "{body}");
    }
}
