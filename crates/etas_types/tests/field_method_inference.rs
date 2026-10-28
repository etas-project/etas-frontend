use etas_core::{DiagnosticCode, TypeDiagnosticCode};
use etas_hir::HirExpr;
use etas_types::Type;

fn check(source: &str) -> (etas_hir::HirProgram, etas_types::TypeOutput) {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    let output = etas_types::check_program(&hir);
    (hir, output)
}

#[test]
fn generic_agent_method_rejects_unsatisfied_spec_for_direct_and_deferred_receivers() {
    for receiver in ["Worker", "compile(^Worker)"] {
        let (_, output) = check(&format!(
            r#"
module app;
import std.compiler.{{compile, CompileError}};
spec Allowed;
agent Worker() -> i32 ![] {{ return 1; }}
impl agent Worker {{ flow wrap<T ~ Allowed>(value: T) -> Option<T> ![] {{ return Some(value); }} }}
flow main() -> Option<string> ![Error<CompileError>] {{
    let worker = {receiver};
    return worker.wrap<string>("not allowed");
}}
"#
        ));
        assert!(
            output
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error),
            "{receiver}: {:?}",
            output.diagnostics
        );
        assert!(
            output
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("Allowed")),
            "{receiver}: {:?}",
            output.diagnostics
        );
    }
}

#[test]
fn captured_error_field_method_retains_the_actual_array_receiver_type() {
    let (hir, output) = check(
        r#"
module app;
import std.compiler.{check, CompileError};
agent Worker() -> i32 { return 1; }
flow main() -> usize {
    match check(^Worker)? {
        Ok(_) => { return 0; },
        Err(error) => { return error.diagnostics.len(); },
    }
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let receiver = hir
        .exprs
        .iter()
        .find_map(|(_, expr)| match expr {
            HirExpr::MethodCall {
                receiver, method, ..
            } if method == "len" => Some(*receiver),
            _ => None,
        })
        .unwrap();
    let ty = output.facts.expr_types[&receiver];
    assert!(matches!(output.store.get(ty), Some(Type::Array(_))));
    assert_eq!(
        output.facts.field_projections[&receiver]
            .last()
            .unwrap()
            .output,
        ty
    );
    assert!(
        !output
            .facts
            .known_std_types
            .support_constraints
            .contains(&ty)
    );
}

#[test]
fn deferred_fields_and_methods_solve_together_across_nested_patterns() {
    let (_, output) = check(
        r#"
module app;
flow source() -> { values: Array<{ name: string }> } ![Error<string>] {
    return { values = [{ name = "ready" }] };
}
flow main() -> usize {
    match source()? {
        Ok(value) => {
            match value.values.get(0) {
                Some(row) => { return row.name.len(); },
                None => { return 0; },
            }
        },
        Err(_) => { return 0; },
    }
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(
        output
            .facts
            .expr_types
            .values()
            .all(|ty| !matches!(output.store.get(*ty), Some(Type::Var(_))))
    );
}

#[test]
fn deferred_method_cannot_treat_a_scalar_field_as_a_length_input() {
    let (_, output) = check(
        r#"
module app;
flow source() -> { value: i32 } ![Error<string>] { return { value = 4 }; }
flow main() -> usize {
    match source()? {
        Ok(record) => { return record.value.len(); },
        Err(_) => { return 0; },
    }
}
"#,
    );
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
