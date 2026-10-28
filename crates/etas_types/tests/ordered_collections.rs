use etas_core::{DiagnosticCode, SourceFile, SourceId, TypeDiagnosticCode};

fn check(source: &str) -> etas_types::TypeOutput {
    let parsed = etas_syntax::parse_program(SourceFile::new(SourceId(7), None, source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    etas_types::check_program(&etas_hir::lower_program(&parsed.value))
}

#[test]
fn ordered_constructor_overloads_accept_checked_nominal_and_nested_comparisons() {
    for construction in [
        "OrderedMap.new<Key, string>(compare).insert(key, \"value\")",
        "OrderedSet.new<Key>(compare).insert(key)",
        "OrderedMap.new<i32, string>().insert(1, \"value\")",
        "OrderedSet.new<i32>().insert(1)",
    ] {
        let output = check(&format!(
            r#"
type Key = {{ parts: (i32, string) }};
flow compare(a: Key, b: Key) -> i32 ![] {{ return 0; }}
flow main() -> unit {{
    let key = Key {{ parts = (1, "nested") }};
    let collection = {construction};
    return;
}}
"#
        ));
        assert!(
            output.diagnostics.is_empty(),
            "{construction}: {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn ordered_comparisons_reject_wrong_inputs_results_arity_and_effects() {
    for constructor in ["OrderedMap.new<i32, string>", "OrderedSet.new<i32>"] {
        for signature in [
            "flow compare(a: string, b: string) -> i32 ![] { return 0; }",
            "flow compare(a: i32) -> i32 ![] { return 0; }",
            "flow compare(a: i32, b: i32) -> string ![] { return \"wrong\"; }",
            "flow compare(a: i32, b: i32) -> i32 ![Console.stdout_write] { return 0; }",
        ] {
            let output = check(&format!(
                "{signature} flow main() -> unit {{ let collection = {constructor}(compare); return; }}"
            ));
            assert!(
                output
                    .diagnostics
                    .iter()
                    .any(|d| d.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)),
                "{constructor}: {signature}: {:?}",
                output.diagnostics
            );
            assert!(
                output
                    .diagnostics
                    .iter()
                    .all(|d| d.primary.span.source == SourceId(7))
            );
        }
    }
}
