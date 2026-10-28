use std::cell::Cell;

use etas_syntax::{
    BodyFormatOptions, BodyParseLimits, BodySyntax, BoundedBodyParseError,
    ControlledBodyFormatError, SourceFile,
};

#[test]
fn controlled_parse_keeps_syntax_errors_distinct_from_interruption() {
    let result = BodySyntax::parse_controlled(
        SourceFile::anonymous("let = ;"),
        BodyParseLimits::default(),
        &|| Err("run cancelled"),
    );
    assert!(matches!(
        result,
        Err(BoundedBodyParseError::Interrupted("run cancelled"))
    ));
    let result = BodySyntax::parse_controlled(
        SourceFile::anonymous("let = ;"),
        BodyParseLimits::default(),
        &|| Ok::<(), ()>(()),
    );
    assert!(matches!(result, Err(BoundedBodyParseError::Syntax(_))));
}

#[test]
fn controlled_formatter_checks_long_trivia_and_preserves_successful_output() {
    let source =
        SourceFile::anonymous(format!("{}return 42;", "// retained comment\n".repeat(256)));
    let syntax = BodySyntax::parse_bounded(source, BodyParseLimits::default()).unwrap();
    let checks = Cell::new(0);
    let result = syntax.format_controlled(BodyFormatOptions::default(), &|| {
        checks.set(checks.get() + 1);
        if checks.get() == 32 {
            Err("format cancelled")
        } else {
            Ok(())
        }
    });
    assert!(matches!(
        result,
        Err(ControlledBodyFormatError::Interrupted("format cancelled"))
    ));
    assert_eq!(checks.get(), 32);
    assert_eq!(
        syntax
            .format_controlled(BodyFormatOptions::default(), &|| Ok::<(), ()>(()))
            .unwrap(),
        syntax.format(BodyFormatOptions::default()).unwrap(),
    );
}
