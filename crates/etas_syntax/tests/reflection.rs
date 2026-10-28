use etas_syntax::{
    AstDump, AstDumpWriter, DumpOptions, Punct, SourceFile, SourceId, TokenKind, ast::*, lex,
    parse_body_contents,
};

fn parse(text: &str) -> etas_syntax::Parse<Block> {
    parse_body_contents(SourceFile::new(SourceId(71), None, text))
}

fn target(text: &str) -> ReflectExpr {
    let parsed = parse(text);
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Some(expr) = parsed.value.final_expr else {
        panic!("missing expression")
    };
    let Expr::Reflect(reflection) = *expr else {
        panic!("expected reflection")
    };
    *reflection
}

#[test]
fn lexes_caret_without_changing_existing_operators() {
    let source = SourceFile::new(SourceId(71), None, "^ ~ ~> ::");
    let tokens: Vec<_> = lex(&source)
        .significant_tokens()
        .map(|token| token.kind.clone())
        .collect();
    assert_eq!(
        tokens,
        vec![
            TokenKind::Punct(Punct::Caret),
            TokenKind::Punct(Punct::Tilde),
            TokenKind::Punct(Punct::TildeArrow),
            TokenKind::Punct(Punct::ColonColon),
            TokenKind::Eof
        ]
    );
}

#[test]
fn member_selection_binds_inside_reflection_but_grouped_fields_are_outside() {
    let reflection = target("^app.Generator.run");
    let ReflectTarget::Path(path) = reflection.target else {
        panic!("path")
    };
    assert_eq!(
        path.segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>(),
        ["app", "Generator", "run"]
    );
    let parsed = parse("(^Generator).body");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Some(Expr::Field(field)) = parsed.value.final_expr.as_deref() else {
        panic!("field")
    };
    assert_eq!(field.field.text, "body");
    assert!(matches!(&*field.receiver, Expr::Reflect(_)));
    let parsed = parse("(^Generator).with_body(syntax)");
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    let Some(Expr::MethodCall(call)) = parsed.value.final_expr.as_deref() else {
        panic!("method")
    };
    assert!(matches!(&*call.receiver, Expr::Reflect(_)));
}

#[test]
fn reflects_ordinary_spec_method_selection_without_call_arguments() {
    for text in ["^candidate::Stage.run", "^candidate::app.Stage<i32>.run"] {
        let reflection = target(text);
        let ReflectTarget::SpecMethod {
            receiver,
            spec,
            spec_args,
            method,
        } = reflection.target
        else {
            panic!("spec selection")
        };
        assert_eq!(receiver.segments[0].text, "candidate");
        assert_eq!(spec.segments.last().unwrap().text, "Stage");
        assert_eq!(method.text, "run");
        assert_eq!(spec_args.len(), usize::from(text.contains('<')));
    }
}

#[test]
fn rejects_calls_indices_and_arbitrary_expressions_as_reflection_targets() {
    for text in [
        "^Generator.run(input)",
        "^Generator()",
        "^candidate::Stage.run(input)",
        "^Normalize<i32>(42)",
        "^values[0]",
        "^(Generator)",
        "^(a + b)",
        "^42",
        "^{}",
        "^",
        "^A.",
        "^a::Stage",
    ] {
        let parsed = parse(text);
        assert!(!parsed.diagnostics.is_empty(), "accepted {text}");
        assert!(
            !matches!(
                parsed.value.final_expr.as_deref(),
                Some(Expr::Call(_)) | Some(Expr::MethodCall(_))
            ),
            "constructed call from invalid reflection {text}"
        );
        assert!(
            parsed
                .diagnostics
                .iter()
                .all(|diagnostic| diagnostic.primary.span.source == SourceId(71))
        );
    }
}

#[test]
fn invalid_call_reflection_recovers_at_the_next_statement() {
    let parsed = parse("let bad = ^Generator.run(input); let good = ^Normalize; good");
    assert_eq!(parsed.diagnostics.len(), 1, "{:#?}", parsed.diagnostics);
    let Stmt::Let(good) = &parsed.value.stmts[1] else {
        panic!("let")
    };
    assert!(matches!(good.value, Expr::Reflect(_)));
    assert_eq!(
        parsed.diagnostics[0].primary.span.range.start.to_usize(),
        10
    );
    assert_eq!(parsed.diagnostics[0].primary.span.range.end.to_usize(), 32);
}

#[test]
fn reflection_dump_preserves_target_kind_and_unicode_spans() {
    let text = "^工作::Stage.run";
    let reflection = target(text);
    assert_eq!(reflection.span.range.end.to_usize(), text.len());
    let mut writer = AstDumpWriter::new(DumpOptions::default());
    reflection.dump(&mut writer);
    let dump = writer.finish();
    assert!(dump.contains("ReflectSpecMethodExpr"), "{dump}");
    assert!(dump.contains("receiver=\"工作\""), "{dump}");
    assert!(dump.contains("spec=Stage"), "{dump}");
}
