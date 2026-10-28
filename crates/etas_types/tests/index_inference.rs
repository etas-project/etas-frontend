use etas_core::{DiagnosticCode, SourceFile, SourceId, TypeDiagnosticCode};
use etas_hir::{HirExpr, HirLiteral, lower_program};
use etas_types::{PrimitiveType, Type, check_program};

#[test]
fn deferred_index_output_constrains_literals_before_numeric_defaulting() {
    for (params, expression) in [
        ("data: bytes", "identity(data)[0] == 255"),
        ("data: bytes", "identity(data)[identity(0)] == 255"),
        ("data: Array<u8>", "identity(data)[0] == 255"),
        ("data: Array<bytes>", "identity(data)[0][0] == 255"),
        ("data: Map<u8, bytes>", "identity(data)[0][0] == 255"),
        (
            "data: std.host.command.CommandResult",
            "identity(data).stdout[0] == 255",
        ),
    ] {
        let source = format!(
            "import std.host.command.CommandResult; flow identity<T>(value: T) -> T {{ return value; }} flow probe({params}) -> bool {{ return {expression}; }}"
        );
        let parsed = etas_syntax::parse_program(SourceFile::new(SourceId(7), None, source));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = lower_program(&parsed.value);
        let output = check_program(&hir);
        assert!(
            output.diagnostics.is_empty(),
            "{expression}: {:?}",
            output.diagnostics
        );
        let comparison_literal = hir
            .exprs
            .iter()
            .find_map(|(id, expr)| {
                matches!(expr, HirExpr::Literal(HirLiteral::Int { text, .. }) if text == "255")
                    .then_some(id)
            })
            .unwrap();
        let literal_type = output.facts.expr_types[&comparison_literal];
        assert_eq!(
            output.store.get(literal_type),
            Some(&Type::Primitive(PrimitiveType::U8))
        );
        assert_eq!(
            output.facts.index_facts.len(),
            expression.matches('[').count()
        );
    }
}

#[test]
fn deferred_index_worklist_propagates_through_long_projection_chains() {
    let levels = 96;
    let ty = format!("{}bytes{}", "Array<".repeat(levels), ">".repeat(levels));
    let expression = format!("identity(data){}[0] == 128", "[0]".repeat(levels));
    let source = format!(
        "flow identity<T>(value: T) -> T {{ return value; }} flow probe(data: {ty}) -> bool {{ return {expression}; }}"
    );
    let parsed = etas_syntax::parse_program(SourceFile::new(SourceId(7), None, source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let output = check_program(&lower_program(&parsed.value));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(output.facts.index_facts.len(), levels + 1);
}

#[test]
fn deferred_index_literals_retain_range_and_concrete_type_checks() {
    for expression in ["identity(data)[0] == 256", "identity(data)[256] == 0"] {
        let params = if expression.contains("[256]") {
            "data: Map<u8, u8>"
        } else {
            "data: bytes"
        };
        let source = format!(
            "flow identity<T>(value: T) -> T {{ return value; }} flow probe({params}) -> bool {{ return {expression}; }}"
        );
        let parsed = etas_syntax::parse_program(SourceFile::new(SourceId(7), None, source));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = lower_program(&parsed.value);
        let output = check_program(&hir);
        let literal_span = hir
            .exprs
            .iter()
            .find_map(|(_, expr)| match expr {
                HirExpr::Literal(HirLiteral::Int { text, span }) if text == "256" => Some(*span),
                _ => None,
            })
            .unwrap();
        assert!(
            output.diagnostics.iter().any(|diagnostic| {
                diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
                    && diagnostic.message.contains("numeric literal `256`")
                    && diagnostic
                        .labels
                        .iter()
                        .any(|label| label.span == literal_span)
            }),
            "{expression}: {:?}",
            output.diagnostics
        );
    }
    for source in [
        "flow identity<T>(value: T) -> T { return value; } flow probe(data: bytes, other: i32) -> bool { return identity(data)[0] == other; }",
        "flow identity<T>(value: T) -> T { return value; } flow probe(data: bytes) -> u8 { return identity(data)[true]; }",
        "flow identity<T>(value: T) -> T { return value; } flow probe(data: bool) -> u8 { return identity(data)[0]; }",
    ] {
        let parsed = etas_syntax::parse_program(SourceFile::new(SourceId(7), None, source));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let output = check_program(&lower_program(&parsed.value));
        assert!(
            output.diagnostics.iter().any(|diagnostic| diagnostic.code
                == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)),
            "{source}: {:?}",
            output.diagnostics
        );
    }
}
