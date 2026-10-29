use etas_core::{DiagnosticCode, EffectDiagnosticCode};
use etas_frontend::{Frontend, ProjectInput, SourceInput};

fn check(source: &str) -> etas_frontend::ProjectOutput {
    Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(source)))
}

#[test]
fn secret_type_and_effect_resolve_independently_of_declaration_order() {
    let identity = "flow identity(value: Secret<i32>) -> Secret<i32> ![] { return value; }";
    let constraint = "spec NoSecrets: trace = -Secret;";
    for declarations in [
        format!("{identity}\n{constraint}"),
        format!("{constraint}\n{identity}"),
    ] {
        let output = check(&format!(
            "module main; {declarations}\nflow protected(value: Secret<i32>) -> Secret<i32> ![] ~ NoSecrets {{ return identity(value); }}\nflow main() -> unit ![] {{ return; }}"
        ));
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        assert!(output.checked.is_some());
    }
}

#[test]
fn broad_secret_trace_constraint_rejects_a_real_secret_action() {
    let output = check(
        r#"
module main;
type ApiKey;
spec NoSecrets: trace = -Secret;
flow identity(value: Secret<i32>) -> Secret<i32> ![] { return value; }
flow protected(key: SecretKey<ApiKey>) -> unit ![Error<SecretError>] ~ NoSecrets {
    std.secret.read(key);
    return;
}
flow main() -> unit ![] { return; }
"#,
    );
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Effect(EffectDiagnosticCode::TraceSpecDenied)),
        "{:?}",
        output.diagnostics
    );
    assert!(
        !output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Effect(EffectDiagnosticCode::IncompleteEffectFacts)),
        "{:?}",
        output.diagnostics
    );
    assert!(output.checked.is_none());
}

#[test]
fn secret_constructor_and_lexical_shadowing_are_not_effect_lookups() {
    let output = check(
        r#"
module main;
spec NoSecrets: trace = -Secret;
flow wrap(value: i32) -> Secret<i32> ![] ~ NoSecrets { return Secret(value); }
flow main() -> i32 ![] {
    let Secret = 42;
    let hidden = wrap(Secret);
    return Secret;
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(output.checked.is_some());
}
