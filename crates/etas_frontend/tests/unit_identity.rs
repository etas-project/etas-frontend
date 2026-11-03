use etas_frontend::{Frontend, ProjectInput, SourceInput};

fn check(source: &str) -> etas_frontend::ProjectOutput {
    Frontend.check_project(ProjectInput::single_source(SourceInput::anonymous(source)))
}

#[test]
fn empty_tuple_is_unit_in_annotations_calls_patterns_and_generic_payloads() {
    let output = check(
        r#"
module app.main;
flow sink(value: unit) -> unit { return value; }
flow identity(value: ()) -> () { return value; }
flow main() -> i32 {
    let pair: (unit, i32) = (identity(()), 7);
    let ((), number) = pair;
    let optional: Option<unit> = Some(());
    let result: Result<(), string> = Ok(());
    match optional { Some(()) => sink(()), None => sink(()) };
    match result { Ok(()) => sink(()), Err(_) => sink(()) };
    match () { () => sink(()) };
    return number;
}
"#,
    );
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = output.checked.expect("checked unit program");
    assert!(
        checked
            .type_store
            .iter()
            .all(|(_, ty)| !matches!(ty, etas_types::Type::Tuple(elems) if elems.is_empty()))
    );
}

#[test]
fn unit_canonicalization_does_not_erase_nominal_or_nonempty_tuple_identity() {
    for source in [
        "module main; flow main() -> unit { return (1, 2); }",
        "module main; type Done = unit; flow main() -> Done { return (); }",
    ] {
        let output = check(source);
        assert!(output.checked.is_none(), "accepted {source}");
        assert!(
            output.diagnostics.iter().any(|diagnostic| diagnostic.code
                == etas_core::DiagnosticCode::Type(etas_core::TypeDiagnosticCode::TypeMismatch)),
            "{:?}",
            output.diagnostics
        );
    }
    let output = check("module main; type Done = unit; flow main() -> Done { return Done(()); }");
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert!(output.checked.is_some());
}
