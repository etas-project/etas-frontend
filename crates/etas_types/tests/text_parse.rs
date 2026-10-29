use etas_core::{Severity, SourceFile, SourceId};
use etas_hir::lower_program;
use etas_types::{PrimitiveType, Type, check_program};

#[test]
fn text_parse_error_uses_registry_nominal_storage() {
    let parsed = etas_syntax::parse_program(SourceFile::new(
        SourceId(0),
        None,
        "import std.text.{parse_i32, ParseError}; flow f() -> Result<i32, ParseError> { return parse_i32(\"invalid\"); }",
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let output = check_program(&lower_program(&parsed.value));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let nominal = output
        .store
        .iter()
        .find_map(|(ty, value)| match value {
            Type::Nominal(nominal) if nominal.name == "std.text.ParseError" => Some((ty, nominal)),
            _ => None,
        })
        .expect("checked nominal ParseError");
    assert!(matches!(
        nominal.1.representation.and_then(|ty| output.store.get(ty)),
        Some(Type::Primitive(PrimitiveType::String))
    ));
    assert!(
        output
            .facts
            .known_std_types
            .intrinsic_only_constructors
            .contains(&nominal.0)
    );
}

#[test]
fn text_parse_error_rejects_source_construction_and_representation_assignment() {
    for body in [
        "flow f() -> ParseError { return ParseError(\"forged\"); }",
        "flow f() -> ParseError { return \"forged\"; }",
        "flow f(value: ParseError) -> string { return value; }",
    ] {
        let parsed = etas_syntax::parse_program(SourceFile::new(
            SourceId(0),
            None,
            format!("import std.text.ParseError; {body}"),
        ));
        assert!(
            parsed.diagnostics.is_empty(),
            "{body}: {:?}",
            parsed.diagnostics
        );
        let output = check_program(&lower_program(&parsed.value));
        assert!(
            output
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error),
            "{body}: {:?}",
            output.diagnostics
        );
    }
}
