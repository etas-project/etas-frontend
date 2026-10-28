use etas_core::{DiagnosticCode, TypeDiagnosticCode};
use etas_frontend::{EntryPolicy, Frontend, ProjectInput, ProjectOutput, SourceInput};
use etas_types::{ItemSignature, PrimitiveType, Type};

fn check(source: &str) -> ProjectOutput {
    let mut source = SourceInput::anonymous(source);
    source.id = etas_core::SourceId(72);
    let mut input = ProjectInput::single_source(source);
    input.options.entry_policy = EntryPolicy::Optional;
    Frontend.check_project(input)
}

fn assert_output(output: &ProjectOutput, name: &str, expected: Type) {
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.as_ref().unwrap();
    let signature = checked
        .types
        .item_signatures
        .iter()
        .find_map(|(item, signature)| {
            let symbol = checked.items[*item].symbol()?;
            (checked.symbols.get(symbol)?.name == name).then_some(signature)
        })
        .unwrap();
    let ItemSignature::Flow(signature) = signature else {
        panic!("flow signature")
    };
    assert_eq!(checked.type_store.get(signature.output), Some(&expected));
}

#[test]
fn explicit_returns_and_final_expressions_produce_checked_outputs() {
    let output = check(
        r#"
flow identity(text: string) { return text; }
flow number() { 42 }
flow empty() {}
flow caller() -> string { return identity("ok"); }
"#,
    );
    assert_output(&output, "identity", Type::Primitive(PrimitiveType::String));
    assert_output(&output, "number", Type::Primitive(PrimitiveType::I32));
    assert_output(&output, "empty", Type::Primitive(PrimitiveType::Unit));
}

#[test]
fn forward_calls_and_mutual_recursion_share_output_constraints() {
    let output = check(
        r#"
flow first(text: string) { return second(text); }
flow second(text: string) { return text; }
flow even(n: i32) { if n == 0 { return true; } return odd(n - 1); }
flow odd(n: i32) { if n == 0 { return false; } return even(n - 1); }
"#,
    );
    assert_output(&output, "first", Type::Primitive(PrimitiveType::String));
    assert_output(&output, "second", Type::Primitive(PrimitiveType::String));
    assert_output(&output, "even", Type::Primitive(PrimitiveType::Bool));
    assert_output(&output, "odd", Type::Primitive(PrimitiveType::Bool));
}

#[test]
fn lambda_returns_do_not_become_outer_flow_return_candidates() {
    let output = check(
        r#"
flow outer() {
    let f: () -> string = () => { return "inner"; };
    return 1;
}
"#,
    );
    assert_output(&output, "outer", Type::Primitive(PrimitiveType::I32));
}

#[test]
fn never_candidates_do_not_override_real_outputs_or_invent_never_output() {
    let output = check(
        r#"
flow partial(condition: bool) {
    if condition { return abort("stop"); }
    return "ok";
}
flow stopped() { return abort("stop"); }
"#,
    );
    assert_output(&output, "partial", Type::Primitive(PrimitiveType::String));
    assert_output(&output, "stopped", Type::Primitive(PrimitiveType::Unit));
}

#[test]
fn incompatible_return_candidates_are_not_widened_or_nominally_unwrapped() {
    for source in [
        "flow bad(condition: bool) { if condition { return 1; } return \"wrong\"; }",
        "type UserId = string; flow bad(condition: bool, id: UserId) { if condition { return id; } return \"wrong\"; }",
        "flow bad(condition: bool) { if condition { return; } return 1; }",
    ] {
        let output = check(source);
        assert!(output.checked.is_none());
        assert!(
            output.diagnostics.iter().any(|diagnostic| diagnostic.code
                == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)),
            "{:?}",
            output.diagnostics
        );
        assert!(
            output
                .diagnostics
                .iter()
                .all(|diagnostic| diagnostic.primary.span.source == etas_core::SourceId(72))
        );
    }
}

#[test]
fn unconstrained_recursive_output_requires_an_annotation() {
    let output = check("flow forever() { return forever(); }");
    assert!(output.checked.is_none());
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Type(TypeDiagnosticCode::IncompleteTypeFacts)),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn explicit_output_still_checks_the_final_expression() {
    let output = check("flow bad() -> i32 { \"wrong\" }");
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
fn incremental_inferred_output_change_invalidates_the_importing_caller() {
    use etas_frontend::{
        CheckRequest, FrontendSession, ProjectChangeSet, ProjectRevision, SourceChange, SourceKind,
        SourceVersion,
    };
    let library = "module app.lib; public flow value() { return 42; }";
    let mut input = ProjectInput::single_source(SourceInput::anonymous(""));
    input.sources = [
        library,
        "module app.main; import app.lib.value; flow main() -> i32 { return value(); }",
    ]
    .into_iter()
    .enumerate()
    .map(|(index, text)| SourceInput {
        id: etas_core::SourceId(index as u32 + 52),
        path: None,
        text: text.into(),
        kind: SourceKind::SourceProjectFile,
    })
    .collect();
    let mut session = FrontendSession::new();
    let project = session.open_project(input);
    let initial = session
        .check(project, CheckRequest::full_project())
        .unwrap();
    assert!(
        initial.output.checked.is_some(),
        "{:?}",
        initial.output.diagnostics
    );
    session
        .apply_changes(
            project,
            ProjectChangeSet {
                revision: ProjectRevision(1),
                source_changes: vec![SourceChange::Replace {
                    source: etas_core::SourceId(52),
                    version: SourceVersion(1),
                    text: library.replace("42", "\"wrong\""),
                }],
                dependency_overlay_changes: vec![],
                environment_change: None,
                option_changes: vec![],
            },
        )
        .unwrap();
    let edited = session.check(project, CheckRequest::incremental()).unwrap();
    assert!(
        edited.output.checked.is_none(),
        "a stale caller body accepted a changed inferred API"
    );
    assert!(
        edited
            .output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code
                == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
                && diagnostic.primary.span.source == etas_core::SourceId(53)),
        "{:?}",
        edited.output.diagnostics
    );
}

#[test]
fn inference_temporaries_do_not_expand_the_shared_signature_universe() {
    let mut retained = Vec::new();
    for count in [1, 32, 256] {
        let mut source = "flow value() {".to_owned();
        for index in 0..count {
            source.push_str(&format!("let v_{index} = ({index}, \"local\");"));
        }
        source.push_str("return 42; } flow main() -> i32 { return value(); }");
        let output = check(&source);
        assert_output(&output, "value", Type::Primitive(PrimitiveType::I32));
        retained.push(output.signature_types.as_ref().unwrap().store.len());
    }
    assert!(
        retained.iter().all(|size| *size == retained[0]),
        "temporary body types leaked into declaration storage: {retained:?}"
    );
}

#[test]
fn inferred_method_outputs_must_satisfy_the_declared_callable_spec() {
    let output = check(
        r#"
spec Stage<I, O>: callable I => O ![];
agent Worker;
impl agent Worker ~ Stage<i32, string> { flow run(value: i32) { return value; } }
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
fn generic_return_inference_preserves_the_declared_binder() {
    let output = check(
        "flow identity<T>(value: T) { return value; } flow main() -> i32 { return identity(42); }",
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.unwrap();
    let signature = checked
        .types
        .item_signatures
        .values()
        .find_map(|signature| match signature {
            ItemSignature::Flow(signature) if !signature.generic_params.is_empty() => {
                Some(signature)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(signature.output, signature.generic_params[0].subject);
}
