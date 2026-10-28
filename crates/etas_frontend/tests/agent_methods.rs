use etas_frontend::{CheckRequest, FrontendSession, ProjectInput, SourceInput};

fn check(source: &str) -> etas_frontend::ProjectOutput {
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(source)));
    session
        .check(project, CheckRequest::full_project())
        .unwrap()
        .output
}

#[test]
fn agent_impl_methods_have_canonical_body_units_and_checked_call_targets() {
    let output = check(
        r#"
module app.main;
agent Worker;
impl agent Worker {
    flow run(value: i32) -> i32 { return value + 1; }
    public flow explain(value: string) -> string { return value; }
}
flow main() -> i32 { return Worker.run(41); }
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.as_ref().expect("checked agent methods");
    assert_eq!(checked.types.agent_methods.len(), 2);
    assert_eq!(checked.types.agent_method_calls.len(), 1);
    let bodies = output.hir_body_bindings.as_ref().unwrap();
    for method in checked.types.agent_methods.values() {
        assert!(bodies.body_items.values().any(|item| *item == method.body));
        assert!(
            output
                .type_body_outputs
                .values()
                .any(|body| body.owner == method.body)
        );
        assert!(checked.effects.item_effects.contains_key(&method.body));
    }
}

#[test]
fn agent_method_body_is_checked_by_the_regular_body_pipeline() {
    let output = check(
        r#"
agent Worker;
impl agent Worker { flow run() -> i32 { return "wrong"; } }
flow main() -> i32 { return Worker(); }
"#,
    );
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("string")),
        "{:?}",
        output.diagnostics
    );
    assert!(output.checked.is_none());
}

#[test]
fn duplicate_inherent_agent_method_is_rejected() {
    let output = check(
        r#"
agent Worker() -> i32 { return 1; }
impl agent Worker { flow run() -> i32 { return 2; } }
flow main() -> i32 { return Worker(); }
"#,
    );
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("duplicate inherent agent method")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn agent_callable_spec_conformance_checks_the_actual_run_signature() {
    for (output_type, valid) in [("i32", true), ("string", false)] {
        let source = format!(
            r#"
spec Pure<I, O>: callable I => O ![];
agent Worker;
impl agent Worker ~ Pure<i32, {output_type}> {{
    flow run(value: i32) -> i32 {{ return value; }}
}}
flow main() -> i32 {{ return Worker(42); }}
"#
        );
        let output = check(&source);
        assert_eq!(output.checked.is_some(), valid, "{:?}", output.diagnostics);
        if valid {
            let facts = &output.types.as_ref().unwrap().facts;
            assert_eq!(facts.callable_spec_satisfactions.len(), 1);
            let entry = facts.agent_methods.values().next().unwrap();
            assert_eq!(facts.callable_spec_satisfactions[0].item, entry.body);
        }
    }
}

#[test]
fn agent_trace_spec_applies_to_the_selected_run_body() {
    let output = check(
        r#"
import std.io.println;
spec Safe: trace = -Console.stdout_write;
agent Worker;
impl agent Worker ~ Safe {
    flow run() -> unit { println("forbidden"); return; }
}
flow main() -> unit { Worker(); return; }
"#,
    );
    assert!(output.checked.is_none());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == etas_core::DiagnosticCode::Effect(etas_core::EffectDiagnosticCode::TraceSpecDenied)),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn callable_spec_rejects_inferred_escaping_effects_and_allows_marker_agent_without_entry() {
    let invalid = check(
        r#"
spec Pure<I, O>: callable I => O ![];
agent Worker;
impl agent Worker ~ Pure<unit, unit> {
    flow run() -> unit { perform Error<i32>.raise(1); return; }
}
flow main() -> unit { return Worker(); }
"#,
    );
    assert!(invalid.checked.is_none());
    assert!(
        invalid
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("callable spec bound")),
        "{:?}",
        invalid.diagnostics
    );
    let valid = check(
        r#"
spec Marker;
agent Worker;
impl agent Worker ~ Marker {}
flow main() -> unit { let definition = ^Worker; return; }
"#,
    );
    assert!(valid.diagnostics.is_empty(), "{:?}", valid.diagnostics);
    assert_eq!(
        valid
            .types
            .as_ref()
            .unwrap()
            .facts
            .type_spec_satisfactions
            .len(),
        1
    );
}
