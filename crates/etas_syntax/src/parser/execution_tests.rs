use std::cell::Cell;

use crate::{BodyParseLimits, BodySyntax, BoundedBodyParseError, SourceFile};

#[test]
fn grammar_interruption_discards_the_recovery_ast_and_original_error_is_preserved() {
    let source = SourceFile::anonymous("let value = 1;".repeat(128));
    let lexical_checks = Cell::new(0);
    crate::lexer::lex_with_checkpoint(&source, &mut Vec::new(), &|| {
        lexical_checks.set(lexical_checks.get() + 1);
        Ok::<(), ()>(())
    })
    .unwrap();
    let stop_at = lexical_checks.get() + 100;
    let checks = Cell::new(0);
    let result = BodySyntax::parse_controlled(source, BodyParseLimits::default(), &|| {
        checks.set(checks.get() + 1);
        if checks.get() == stop_at {
            Err("interrupted inside grammar")
        } else {
            Ok(())
        }
    });
    assert!(matches!(
        result,
        Err(BoundedBodyParseError::Interrupted(
            "interrupted inside grammar"
        ))
    ));
    assert_eq!(
        checks.get(),
        stop_at,
        "recovery must not restart the cancelled work"
    );
}

#[test]
fn long_single_tokens_check_cancellation_during_lexing() {
    for text in [
        " ".repeat(8192),
        format!("// {}", "x".repeat(8192)),
        format!("/* {} */", "x".repeat(8192)),
        "identifier".repeat(1024),
        "1234567890".repeat(1024),
        format!("0x{}", "a".repeat(8192)),
        format!("\"{}\"", "x".repeat(8192)),
        format!("'{}'", "x".repeat(8192)),
    ] {
        let checks = Cell::new(0);
        let mut diagnostics = Vec::new();
        let result = crate::lexer::lex_with_checkpoint(
            &SourceFile::anonymous(text),
            &mut diagnostics,
            &|| {
                checks.set(checks.get() + 1);
                if checks.get() == 64 {
                    Err("lexical cancellation")
                } else {
                    Ok(())
                }
            },
        );
        assert!(matches!(result, Err("lexical cancellation")));
        assert_eq!(checks.get(), 64);
        assert!(diagnostics.is_empty());
    }
}
