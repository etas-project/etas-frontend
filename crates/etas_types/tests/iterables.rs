use etas_core::{DiagnosticCode, SourceFile, SourceId, TypeDiagnosticCode};
use etas_types::check_program;

fn check(source: &str) -> etas_types::TypeOutput {
    let parsed = etas_syntax::parse_program(SourceFile::new(SourceId(7), None, source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    check_program(&hir)
}

#[test]
fn map_iteration_binds_entry_pair_key_and_value_types() {
    for source in [
        r#"
flow accept(key: string, value: i32) -> unit { return; }
flow main(entries: Map<string, i32>) -> unit {
    for (key, value) in entries limit Iterations(8) { accept(key, value); }
}
"#,
        r#"
flow main<K, V>(entries: Map<K, V>) -> unit {
    for entry in entries limit Iterations(8) { let pair: (K, V) = entry; }
    for (key, value) in entries limit Iterations(8) {
        let expected_key: K = key;
        let expected_value: V = value;
    }
}
"#,
        r#"
type Key = string;
type Value = i32;
flow main(entries: Map<Key, Value>) -> unit {
    for (key, value) in entries limit Iterations(8) {
        let expected_key: Key = key;
        let expected_value: Value = value;
    }
}
"#,
    ] {
        let output = check(source);
        assert!(
            output.diagnostics.is_empty(),
            "{source}\n{:?}",
            output.diagnostics
        );
    }
}

#[test]
fn map_iteration_rejects_wrong_key_value_or_entry_pattern() {
    for (body, code) in [
        (
            "for (key, value) in entries limit Iterations(8) { let wrong: i32 = key; }",
            TypeDiagnosticCode::Mismatch,
        ),
        (
            "for (key, value) in entries limit Iterations(8) { let wrong: string = value; }",
            TypeDiagnosticCode::Mismatch,
        ),
        (
            "for (key, value, extra) in entries limit Iterations(8) {}",
            TypeDiagnosticCode::TypeMismatch,
        ),
        (
            "for entry in entries limit Iterations(8) { let wrong: i32 = entry; }",
            TypeDiagnosticCode::Mismatch,
        ),
    ] {
        let output = check(&format!(
            "flow main(entries: Map<string, i32>) -> unit {{ {body} }}"
        ));
        let code = DiagnosticCode::Type(code);
        assert!(
            output.diagnostics.iter().any(|d| d.code == code),
            "{body}\n{:?}",
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
