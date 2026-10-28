use etas_types::{EffectArgRef, Type};

fn check(source: &str) -> etas_types::TypeOutput {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    assert!(hir.diagnostics.is_empty(), "{:?}", hir.diagnostics);
    etas_types::check_program(&hir)
}

#[test]
fn action_specialization_preserves_applied_types_and_nested_schema_payloads() {
    let output = check(
        r#"
enum Tree<T> { Leaf(T), Branch(Tree<T>, Tree<T>), }
effect Query { action choose<Output>(schema: Schema<Output>) -> Output; }
flow selected(schema: Schema<Tree<string>>) -> Tree<string> {
    return perform Query.choose<Tree<string>>(schema) with {
        Query.choose<Tree<string>>(_) => { resume Tree.Leaf("checked"); }
    };
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let handlers = output.facts.handler_actions.values().collect::<Vec<_>>();
    let [fact] = handlers.as_slice() else {
        panic!("one checked handler");
    };
    assert!(
        matches!(output.store.get(fact.output), Some(Type::Applied { args, .. })
        if matches!(output.store.get(args[0]), Some(Type::Primitive(etas_types::PrimitiveType::String))))
    );
    assert_eq!(
        output.store.get(fact.params[0]),
        Some(&Type::Schema(fact.output))
    );
    assert_eq!(fact.selectors, vec![EffectArgRef::Type(fact.output)]);
}

#[test]
fn action_specialization_rejects_a_different_applied_resume_payload() {
    let output = check(
        r#"
enum Tree<T> { Leaf(T), }
effect Query { action choose<Output>(schema: Schema<Output>) -> Output; }
flow selected(schema: Schema<Tree<string>>) -> Tree<string> {
    return perform Query.choose<Tree<string>>(schema) with {
        Query.choose<Tree<string>>(_) => { resume Tree.Leaf(true); }
    };
}
"#,
    );
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == etas_core::DiagnosticCode::Type(etas_core::TypeDiagnosticCode::TypeMismatch)),
        "{:?}",
        output.diagnostics
    );
}
