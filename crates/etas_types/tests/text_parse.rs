use etas_core::{Severity, SourceFile, SourceId};
use etas_hir::lower_program;
use etas_types::{Type, check_program};

#[test]
fn text_parse_error_uses_the_registry_text_error_enum() {
    let parsed = etas_syntax::parse_program(SourceFile::new(
        SourceId(0),
        None,
        "import std.text.{parse_i32, TextError}; flow f() -> Result<i32, TextError> { return parse_i32(\"invalid\"); }",
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let output = check_program(&lower_program(&parsed.value));
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let (error, enumeration) = output
        .store
        .iter()
        .find_map(|(ty, value)| match value {
            Type::Enum(enumeration) if enumeration.name == "std.text.TextError" => {
                Some((ty, enumeration))
            }
            _ => None,
        })
        .expect("checked TextError enum");
    assert_eq!(output.facts.known_std_types.text_error, Some(error));
    assert_eq!(enumeration.name, "std.text.TextError");
}

#[test]
fn removed_text_parse_error_does_not_resolve() {
    let parsed = etas_syntax::parse_program(SourceFile::new(
        SourceId(0),
        None,
        "import std.text.ParseError; flow f() -> ParseError { return ParseError; }",
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let output = check_program(&lower_program(&parsed.value));
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error),
        "{:?}",
        output.diagnostics
    );
}
