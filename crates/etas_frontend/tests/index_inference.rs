use etas_core::{DiagnosticCode, TypeDiagnosticCode};
use etas_frontend::{Frontend, SourceInput};

#[test]
fn checked_project_keeps_deferred_index_literal_types_and_effect_facts() {
    let output = Frontend.check(SourceInput::anonymous(
        "flow identity<T>(value: T) -> T { return value; } flow is_zero(data: bytes) -> bool { let alias = identity(data); return alias[0] == 0; } flow main() -> unit { return; }",
    ));
    assert!(output.checked.is_some(), "{:?}", output.diagnostics);
    let checked = output.checked.as_ref().unwrap();
    assert_eq!(checked.types.index_facts.len(), 1);
    assert!(output.effects.is_some());
}

#[test]
fn checked_project_rejects_out_of_range_deferred_comparison_literal() {
    let output = Frontend.check(SourceInput::anonymous(
        "flow identity<T>(value: T) -> T { return value; } flow is_zero(data: bytes) -> bool { let alias = identity(data); return alias[0] == 256; } flow main() -> unit { return; }",
    ));
    assert!(output.checked.is_none());
    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
                && diagnostic.message.contains("numeric literal `256`")
        }),
        "{:?}",
        output.diagnostics
    );
}
