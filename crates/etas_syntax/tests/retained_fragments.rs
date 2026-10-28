use etas_syntax::{
    BodyFormatOptions, BodyParseLimits, BoundedBodyParseError, RetainedBodyFragment,
    RetainedFragmentForm as F, SourceBodyView, SourceFile, SourceId,
};

#[test]
fn retained_fragments_preserve_body_forms_handlers_and_relocatable_spans() {
    for (form, source, expression, handled) in [
        (F::BlockContents, "return value;", false, false),
        (F::Expression, "value + 1", true, false),
        (
            F::HandledBlock,
            "{ return value; } with { Error<i32>.raise(_) => { finish 0; } }",
            false,
            true,
        ),
        (
            F::HandledExpression,
            "value + 1; with { Error<i32>.raise(_) => { finish 0; } }",
            true,
            true,
        ),
    ] {
        let syntax = RetainedBodyFragment::parse_controlled(
            SourceFile::new(SourceId(7), None, source),
            form,
            BodyParseLimits::default(),
            &|| Ok::<(), ()>(()),
        )
        .unwrap();
        assert_eq!(
            matches!(syntax.body(), SourceBodyView::Expression(_)),
            expression
        );
        assert_eq!(syntax.trailing_handler().is_some(), handled);
        let rendered = syntax
            .format_controlled(BodyFormatOptions::default(), &|| Ok::<(), ()>(()))
            .unwrap();
        assert!(
            etas_syntax::BodySyntax::parse(SourceFile::anonymous(rendered.clone())).is_ok(),
            "{rendered}"
        );
        if handled {
            assert!(rendered.starts_with("handle {"), "{rendered}");
        }
        let relocated = etas_syntax::RelocatedBodySyntax::new(
            syntax.body(),
            syntax.trailing_handler(),
            syntax.source(),
            SourceId(8),
            Default::default(),
        )
        .unwrap();
        assert_eq!(relocated.source().id, SourceId(8));
    }
}

#[test]
fn retained_fragment_forms_do_not_accept_recovered_or_trailing_source() {
    for (form, text) in [
        (F::Expression, "return 1;"),
        (F::Expression, "1; hidden()"),
        (F::HandledBlock, "return 1;"),
        (F::HandledExpression, "1;"),
        (F::BlockContents, "{ return 1; } with"),
    ] {
        assert!(
            matches!(
                RetainedBodyFragment::parse_controlled(
                    SourceFile::anonymous(text),
                    form,
                    BodyParseLimits::default(),
                    &|| Ok::<(), ()>(())
                ),
                Err(BoundedBodyParseError::Syntax(_))
            ),
            "{form:?}: {text}"
        );
    }
}

#[test]
fn retained_fragment_parsing_enforces_limits_and_cancellation() {
    let limits = BodyParseLimits {
        max_source_bytes: 4,
        ..Default::default()
    };
    assert!(matches!(
        RetainedBodyFragment::parse_controlled(
            SourceFile::anonymous("return 1;"),
            F::BlockContents,
            limits,
            &|| Ok::<(), ()>(())
        ),
        Err(BoundedBodyParseError::Resource(_))
    ));
    let source = format!("{}1{}", "(".repeat(32), ")".repeat(32));
    let limits = BodyParseLimits {
        max_recursion: 8,
        ..Default::default()
    };
    assert!(matches!(
        RetainedBodyFragment::parse_controlled(
            SourceFile::anonymous(source),
            F::Expression,
            limits,
            &|| Ok::<(), ()>(())
        ),
        Err(BoundedBodyParseError::Resource(_))
    ));
    assert!(matches!(
        RetainedBodyFragment::parse_controlled(
            SourceFile::anonymous("return 1;"),
            F::BlockContents,
            BodyParseLimits::default(),
            &|| Err("cancelled")
        ),
        Err(BoundedBodyParseError::Interrupted("cancelled"))
    ));
}
