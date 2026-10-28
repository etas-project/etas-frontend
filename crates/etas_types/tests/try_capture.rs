use etas_core::{DiagnosticCode, SourceFile, SourceId, TypeDiagnosticCode};
use etas_types::{Type, TypeOutput};

fn check(source: &str) -> TypeOutput {
    let parsed = etas_syntax::parse_program(SourceFile::new(SourceId(7), None, source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    etas_types::check_program(&etas_hir::lower_program(&parsed.value))
}

#[test]
fn inferred_try_error_comes_from_the_checked_callable_contract() {
    let output = check(
        r#"
type ReadError = { message: string };
flow read() -> i32 ![Error<ReadError>] { return abort("test"); }
flow main() -> string {
    match read()? {
        Ok(_) => { return "ok"; },
        Err(error) => { return error.message; },
    }
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let fact = output.facts.try_facts.values().next().unwrap();
    assert!(matches!(
        output.store.get(fact.target_error.unwrap()),
        Some(Type::Nominal(_))
    ));
}

#[test]
fn inferred_try_error_uses_call_local_generic_bindings() {
    let output = check(
        r#"
type FirstError = { first: string };
type SecondError = { second: bool };
flow read<E>(error: E) -> unit ![Error<E>] { perform Error<E>.raise(error); }
flow main() -> string {
    let first = read(FirstError { first = "first" })?;
    let second = read(SecondError { second = true })?;
    match (first, second) {
        (Err(a), Err(b)) => { if b.second { return a.first; } return "false"; },
        _ => { return "ok"; },
    }
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let errors: std::collections::HashSet<_> = output
        .facts
        .try_facts
        .values()
        .map(|fact| fact.target_error.unwrap())
        .collect();
    assert_eq!(errors.len(), 2);
}

#[test]
fn inferred_try_error_uses_the_instantiated_effect_row() {
    let output = check(
        r#"
type ReadError = { message: string };
flow read() -> i32 ![Error<ReadError>] { return abort("test"); }
flow invoke<effect E>(f: () -> i32 ![E]) -> i32 ![E] { return f(); }
flow main() -> string {
    match invoke<![Error<ReadError>]>(read)? {
        Ok(_) => { return "ok"; },
        Err(error) => { return error.message; },
    }
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let fact = output.facts.try_facts.values().next().unwrap();
    assert!(matches!(
        output.store.get(fact.target_error.unwrap()),
        Some(Type::Nominal(_))
    ));
}

#[test]
fn try_error_annotation_cannot_change_the_captured_error_type() {
    let output = check(
        r#"
flow read() -> i32 ![Error<string>] { return abort("test"); }
flow main() -> Result<i32, bool> { return read()?; }
"#,
    );
    assert!(
        output.diagnostics.iter().any(|diagnostic| diagnostic.code
            == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)
            && diagnostic.message.contains("Result error type")),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn a_caller_row_parameter_with_the_same_name_is_not_a_substitution_cycle() {
    let output = check(
        r#"
flow invoke<T, effect E>(f: () -> i32 ![E]) -> i32 ![Error<T>, E] { return f(); }
flow capture<effect E>(f: () -> i32 ![E]) -> Result<i32, string> ![E] {
    return invoke<string, ![E]>(f)?;
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
}
