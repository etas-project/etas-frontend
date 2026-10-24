use etas_core::{DiagnosticCode, SourceFile, SourceId, TypeDiagnosticCode};
use etas_types::{Type, TypeId};

fn check(source: &str) -> etas_types::TypeOutput {
    let parsed = etas_syntax::parse_program(SourceFile::new(SourceId(0), None, source));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    etas_types::check_program(&etas_hir::lower_program(&parsed.value))
}

#[test]
fn std_iterable_element_types_survive_aliases_generic_calls_and_finalization() {
    for name in ["Deque", "Queue", "Stack"] {
        for source in [
            format!(
                "flow first<T>(xs: {name}<T>) -> Option<T> {{ for x in xs limit Iterations(1) {{ return Some(x); }} return None; }} flow main(xs: {name}<string>) -> Option<string> {{ return first(xs); }}"
            ),
            format!(
                "alias Values<T> = {name}<T>; flow main(xs: Values<string>) -> string {{ for x in xs limit Iterations(1) {{ return x; }} return \"\"; }}"
            ),
        ] {
            let output = check(&source);
            assert!(
                output.diagnostics.is_empty(),
                "{source}: {:?}",
                output.diagnostics
            );
            let expected = format!("std.collections.{name}");
            let (constructor, fact) = output
                .facts
                .known_std_types
                .iterables
                .iter()
                .find(|(ty, _)| etas_types::ty::display_type(&output.store, **ty) == expected)
                .unwrap();
            assert_eq!(fact.arity, 1);
            assert_eq!(fact.element_parameter, 0);
            assert!(output.store.iter().any(|(_, ty)| matches!(ty, Type::Applied { constructor: applied, .. } if TypeId(applied.0) == *constructor)));
        }
    }
}

#[test]
fn std_iterable_checks_item_type_and_does_not_project_unrelated_nominals() {
    for name in ["Deque", "Queue", "Stack"] {
        for source in [
            format!(
                "flow main(xs: {name}<string>) -> i32 {{ for x in xs limit Iterations(1) {{ return x; }} return 0; }}"
            ),
            format!(
                "type {name}<T> = {{ item: T }}; flow main(xs: {name}<i32>) -> unit {{ for x in xs limit Iterations(1) {{ }} return; }}"
            ),
            format!(
                "type Wrapped<T> = {name}<T>; flow main(xs: Wrapped<i32>) -> unit {{ for x in xs limit Iterations(1) {{ }} return; }}"
            ),
        ] {
            let output = check(&source);
            assert!(
                output
                    .diagnostics
                    .iter()
                    .any(|d| d.code == DiagnosticCode::Type(TypeDiagnosticCode::TypeMismatch)),
                "{source}: {:?}",
                output.diagnostics
            );
        }
    }
}
