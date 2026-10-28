use etas_core::{DiagnosticCode, TypeDiagnosticCode};
use etas_frontend::{Frontend, ProjectInput, SourceInput};

fn check(source: &str) -> etas_frontend::ProjectOutput {
    Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(
        format!("module main; import std.compiler.{{AgentDef, CompileError, compile}}; effect Echo {{ action value<T>(payload: T) -> T; }} {source}"),
    )))
}

fn accepted(source: &str) {
    let output = check(source);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    assert!(checked.types.deferred_handler_instantiations.is_empty());
}

#[test]
fn an_enclosing_generic_with_the_same_name_is_not_a_universal_handler_parameter() {
    for body in [
        "let first = perform Echo.value(7); perform Echo.value(\"text\"); return;",
        "hidden(); return;",
    ] {
        let output = check(&format!(
            r#"
flow hidden() -> string ![Echo.value] {{ return perform Echo.value("text"); }}
flow bad<T>(value: T) -> unit {{
    return handle {{ {body} }} with {{ Echo.value(_) => {{ resume value; }} }};
}}
flow main() -> unit {{ return bad(7); }}
"#,
        ));
        assert!(
            output.checked.is_none(),
            "accepted unrelated binder: {body}"
        );
        assert!(
            output.diagnostics.iter().any(|diagnostic| diagnostic.code
                == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)),
            "{:?}",
            output.diagnostics
        );
    }
}

#[test]
fn a_handler_can_remain_universal_across_distinct_request_types() {
    accepted(
        r#"
flow hidden() -> string ![Echo.value] { return perform Echo.value("text"); }
flow main() -> unit {
    handle { perform Echo.value(7); hidden(); return; } with {
        Echo.value(payload) => { resume payload; }
    };
    return;
}
"#,
    );
}

#[test]
fn an_indirect_compiler_request_cannot_be_hidden_by_a_direct_request() {
    let output = check(
        r#"
agent Worker() -> i32 { return 7; }
agent Other() -> i32 { return 9; }
flow hidden() -> Other ![Error<CompileError>] { return compile(^Other); }
flow main() -> Other ![Error<CompileError>] {
    return handle { let first = compile(^Worker); hidden() } with {
        Code.compile(_) => { resume Worker; }
    };
}
"#,
    );
    assert!(output.checked.is_none());
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code
                == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn prior_requests_do_not_pollute_a_pure_invocation_footprint() {
    accepted(
        r#"
flow hidden() -> i32 ![Echo.value] { return perform Echo.value(7); }
flow main() -> i32 {
    return handle {
        let first = hidden();
        let text = std.text.lowercase("UNCHANGED");
        hidden()
    } with { Echo.value(_) => { resume 42; } };
}
"#,
    );
}

#[test]
fn a_handler_specializes_from_a_source_wrapper_request() {
    accepted(
        r#"
flow hidden() -> i32 ![Echo.value] { return perform Echo.value(7); }
flow main() -> i32 {
    return handle { hidden() } with { Echo.value(_) => { resume 42; } };
}
"#,
    );
}

#[test]
fn request_bindings_are_substituted_across_generic_source_wrappers() {
    accepted(
        r#"
flow hidden<Content>(value: Content) -> Content ![Echo.value] { return perform Echo.value(value); }
flow forward<Item>(value: Item) -> Item ![Echo.value] { return hidden(value); }
flow main() -> i32 {
    return handle { forward(7) } with { Echo.value(_) => { resume 42; } };
}
"#,
    );
}

#[test]
fn request_bindings_follow_a_checked_generic_callable_reference() {
    accepted(
        r#"
flow hidden<Content>(value: Content) -> Content ![Echo.value] { return perform Echo.value(value); }
flow main() -> i32 {
    let selected: (i32) -> i32 ![Echo.value] = hidden;
    return handle { selected(7) } with { Echo.value(_) => { resume 42; } };
}
"#,
    );
}

#[test]
fn requests_inside_an_invoked_lambda_are_visible_to_the_handler() {
    accepted(
        r#"
flow main() -> i32 {
    let selected = () => perform Echo.value(7);
    return handle { selected() } with { Echo.value(_) => { resume 42; } };
}
"#,
    );
}

#[test]
fn requests_follow_callback_parameter_bindings() {
    accepted(
        r#"
flow apply<effect E>(callback: () -> i32 ![E]) -> i32 ![E] { return callback(); }
flow main() -> i32 {
    return handle { apply(() => perform Echo.value(7)) } with { Echo.value(_) => { resume 42; } };
}
"#,
    );
}

#[test]
fn requests_follow_a_returned_closures_captured_callback() {
    accepted(
        r#"
flow bind<effect E>(callback: () -> i32 ![E]) -> () -> i32 ![E] {
    return () => callback();
}
flow apply<effect E>(callback: () -> i32 ![E]) -> i32 ![E] { return callback(); }
flow main() -> i32 {
    let closed = bind(() => perform Echo.value(7));
    return handle { apply(closed) } with { Echo.value(_) => { resume 42; } };
}
"#,
    );
}

#[test]
fn every_invoked_callback_must_agree_with_the_handlers_resume_type() {
    for payload in ["7", "\"wrong\""] {
        let output = check(&format!(
            r#"
flow both<effect E, effect F>(first: () -> i32 ![E], second: () -> i32 ![F]) -> i32 ![E, F] {{
    first(); return second();
}}
flow main() -> i32 {{
    return handle {{ both(
        () => {{ perform Echo.value(7); return 0; }},
        () => {{ perform Echo.value({payload}); return 0; }},
    ) }} with {{ Echo.value(_) => {{ resume 42; }} }};
}}
"#
        ));
        if payload == "7" {
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            assert!(output.checked.is_some());
        } else {
            assert!(output.checked.is_none());
            assert!(
                output.diagnostics.iter().any(|diagnostic| diagnostic.code
                    == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)),
                "{:?}",
                output.diagnostics
            );
        }
    }
}

#[test]
fn an_inner_total_handler_removes_only_its_own_requests() {
    accepted(
        r#"
flow hidden() -> i32 ![Echo.value] { return perform Echo.value(7); }
flow main() -> i32 {
    return handle {
        let inner = handle { perform Echo.value("text") } with {
            Echo.value(payload) => { resume payload; }
        };
        hidden()
    } with { Echo.value(_) => { resume 42; } };
}
"#,
    );
}

#[test]
fn a_recursive_request_graph_terminates_without_a_depth_fallback() {
    accepted(
        r#"
flow descend(count: i32) -> i32 ![Echo.value] {
    if count == 0 { return perform Echo.value(7); }
    return descend(count - 1);
}
flow main() -> i32 {
    return handle { descend(4) } with { Echo.value(_) => { resume 42; } };
}
"#,
    );
}

#[test]
fn a_wrapper_request_of_a_different_type_cannot_resume_a_narrower_value() {
    let output = check(
        r#"
flow hidden() -> string ![Echo.value] { return perform Echo.value("wrong"); }
flow main() -> string {
    return handle { hidden() } with { Echo.value(_) => { resume 42; } };
}
"#,
    );
    assert!(output.checked.is_none());
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code
                == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn compiler_action_handlers_infer_through_source_wrappers() {
    accepted(
        r#"
agent Worker() -> i32 { return 7; }
flow build<A>(definition: AgentDef<A>) -> A ![Error<CompileError>] { return compile(definition); }
flow main() -> Worker ![Error<CompileError>] {
    return handle { build(^Worker) } with { Code.compile(_) => { resume Worker; } };
}
"#,
    );
}

#[test]
fn changing_a_wrappers_request_type_revalidates_a_reused_handler_body() {
    use etas_frontend::{
        CheckRequest, FrontendSession, ProjectChangeSet, ProjectRevision, SourceChange,
        SourceVersion,
    };
    use std::sync::Arc;

    let source = r#"
module main;
effect Echo { action value<T>(payload: T) -> T; }
flow hidden() -> i32 ![Echo.value] { perform Echo.value(7      ); return 0; }
flow main() -> i32 { return handle { hidden() } with { Echo.value(_) => { resume 42; } }; }
"#;
    let mut session = FrontendSession::new();
    let project = session.open_project(ProjectInput::single_source(SourceInput::anonymous(source)));
    let initial = session
        .check(project, CheckRequest::full_project())
        .unwrap();
    assert!(
        initial.output.diagnostics.is_empty(),
        "{:?}",
        initial.output.diagnostics
    );
    let checked = initial.output.checked.as_ref().unwrap();
    let main = checked
        .hir
        .items
        .iter()
        .find_map(|(id, item)| {
            let etas_hir::HirItem::Flow(flow) = item else {
                return None;
            };
            (checked.hir.symbols.get(flow.symbol).unwrap().name == "main").then_some(id)
        })
        .unwrap();
    let before = initial
        .output
        .type_body_outputs
        .values()
        .find(|body| body.owner == main)
        .unwrap();
    session
        .apply_changes(
            project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: vec![SourceChange::Replace {
                    source: etas_core::SourceId(0),
                    version: SourceVersion(1),
                    text: source.replace("value(7      )", "value(\"wrong\")"),
                }],
                dependency_overlay_changes: Vec::new(),
                environment_change: None,
                option_changes: Vec::new(),
            },
        )
        .unwrap();
    let edited = session.check(project, CheckRequest::incremental()).unwrap();
    let reused = edited
        .output
        .type_body_outputs
        .values()
        .find(|body| body.owner == main)
        .unwrap();
    assert!(
        Arc::ptr_eq(before, reused),
        "unchanged handler body must reuse its provisional type artifact"
    );
    assert!(edited.output.checked.is_none());
    assert!(
        edited
            .output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code
                == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)),
        "{:?}",
        edited.output.diagnostics
    );
}
