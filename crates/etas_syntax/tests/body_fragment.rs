use etas_syntax::{
    SourceFile, SourceId, TokenKind,
    ast::{Expr, FlowBody, Item, Stmt},
    parse_body_contents, parse_program,
};

fn source(text: &str) -> SourceFile {
    SourceFile::new(SourceId(91), Some("generated/body.es".into()), text)
}

#[test]
fn parses_body_statements_and_tail_without_synthetic_declaration() {
    let text = "let x = 40;\nvar y = x;\ny = y + 2;\ny";
    let parsed = parse_body_contents(source(text));
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert_eq!(parsed.value.stmts.len(), 3);
    assert!(matches!(
        parsed.value.final_expr.as_deref(),
        Some(Expr::Path(_))
    ));
    assert_eq!(parsed.value.span.source, SourceId(91));
    assert_eq!(parsed.value.span.range.start.to_usize(), 0);
    assert_eq!(parsed.value.span.range.end.to_usize(), text.len());
    assert_eq!(parsed.tokens.tokens.last().unwrap().kind, TokenKind::Eof);
    for stmt in &parsed.value.stmts {
        assert_eq!(stmt.span().source, SourceId(91));
        assert!(stmt.span().range.end.to_usize() <= text.len());
    }
}

#[test]
fn fragment_and_regular_block_use_identical_body_grammar() {
    let body = "let x = [1, 2]; if true { return x[0]; } else { println(\"no\"); } return 0;";
    let fragment = parse_body_contents(source(body));
    let regular = parse_program(source(&format!("flow f() -> i32 {{ {body} }}")));
    assert!(
        fragment.diagnostics.is_empty(),
        "{:#?}",
        fragment.diagnostics
    );
    assert!(regular.diagnostics.is_empty(), "{:#?}", regular.diagnostics);
    let Item::Flow(flow) = &regular.value.items[0].item else {
        panic!("expected flow")
    };
    let FlowBody::Block(block) = &flow.body else {
        panic!("expected block")
    };
    assert_eq!(fragment.value.stmts.len(), block.stmts.len());
    for (fragment, regular) in fragment.value.stmts.iter().zip(&block.stmts) {
        assert_eq!(
            std::mem::discriminant(fragment),
            std::mem::discriminant(regular)
        );
    }
    assert!(matches!(fragment.value.stmts[1], Stmt::If(_)));
    assert!(matches!(fragment.value.stmts[2], Stmt::Return(_)));
}

#[test]
fn parses_empty_comment_only_and_nested_block_bodies() {
    for text in ["", " // comment\n /* body */ "] {
        let parsed = parse_body_contents(source(text));
        assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
        assert!(parsed.value.stmts.is_empty());
        assert!(parsed.value.final_expr.is_none());
        assert_eq!(parsed.value.span.range.end.to_usize(), text.len());
    }
    let parsed = parse_body_contents(source("{ let x = 42; x }"));
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert!(matches!(
        parsed.value.final_expr.as_deref(),
        Some(Expr::Block(_))
    ));
}

#[test]
fn rejects_trailing_garbage_and_unmatched_fragment_delimiters() {
    for text in [
        "42 99",
        "42 }",
        "}",
        "return 42; }",
        "42; flow f() {}",
        "let x = ;",
        "return (42;",
        "/* unclosed",
    ] {
        let parsed = parse_body_contents(source(text));
        assert!(!parsed.diagnostics.is_empty(), "accepted {text:?}");
        for diagnostic in &parsed.diagnostics {
            assert_eq!(diagnostic.primary.span.source, SourceId(91));
            assert!(diagnostic.primary.span.range.end.to_usize() <= text.len());
        }
    }
}

#[test]
fn preserves_original_unicode_offsets_and_editor_eof_diagnostics() {
    let text = "let name = \"工作\";\nreturn name";
    let parsed = parse_body_contents(source(text));
    assert!(!parsed.diagnostics.is_empty());
    let Stmt::Let(stmt) = &parsed.value.stmts[0] else {
        panic!("expected let")
    };
    let span = stmt.value.span();
    assert_eq!(
        &text[span.range.start.to_usize()..span.range.end.to_usize()],
        "\"工作\""
    );
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.primary.span.range.start.to_usize() == text.len())
    );
}
