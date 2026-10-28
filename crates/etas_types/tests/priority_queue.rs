use etas_core::{DiagnosticCode, SourceFile, SourceId, TypeDiagnosticCode};

fn check(source: &str) -> etas_types::TypeOutput {
    let parsed = etas_syntax::parse_program(SourceFile::new(SourceId(7), None, source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    etas_types::check_program(&etas_hir::lower_program(&parsed.value))
}

#[test]
fn priority_queue_overload_uses_declared_not_occurrence_generic_parameter_order() {
    for construction in [
        "PriorityQueue.new<string, i32>(compare)",
        "PriorityQueue.new<string, i32>()",
    ] {
        let output = check(&format!(
            r#"
flow compare(a: i32, b: i32) -> i32 ![] {{ return 0; }}
flow main() -> PriorityQueue<string, i32> {{ return {construction}.push("ok", 1); }}
"#
        ));
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    }
}

#[test]
fn priority_queue_comparison_rejects_wrong_parameters_result_and_effects() {
    for signature in [
        "flow compare(a: string, b: string) -> i32 ![] { return 0; }",
        "flow compare(a: i32) -> i32 ![] { return 0; }",
        "flow compare(a: i32, b: i32) -> string ![] { return \"wrong\"; }",
        "flow compare(a: i32, b: i32) -> i32 ![Console.stdout_write] { return 0; }",
    ] {
        let output = check(&format!(
            "{signature} flow main() -> unit {{ let queue = PriorityQueue.new<string, i32>(compare); return; }}"
        ));
        assert!(
            output
                .diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)),
            "{signature}: {:?}",
            output.diagnostics
        );
        assert!(
            output
                .diagnostics
                .iter()
                .all(|d| d.primary.span.source == SourceId(7)),
            "{:?}",
            output.diagnostics
        );
    }
}
