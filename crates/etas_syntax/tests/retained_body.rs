use etas_syntax::{
    BodyFormatOptions, BodyParseLimits, ProgramBodySelection, RetainedBodyError, SourceBodySyntax,
    SourceBodyView, SourceFile, SourceId, ast,
};
use std::sync::Arc;

#[test]
fn retained_block_is_a_borrowed_ast_and_braces_free_token_view() {
    let source = SourceFile::new(
        SourceId(91),
        None,
        "module app; agent Worker(x: i32) -> i32 { if x > 0 { return x; } return 0; } flow unused() -> unit { return; }",
    );
    let parsed = Arc::new(etas_syntax::parse_program(source.clone()));
    let body = SourceBodySyntax::new(
        source.clone(),
        parsed.clone(),
        ProgramBodySelection {
            item: 0,
            impl_member: None,
        },
        BodyParseLimits::default(),
    )
    .unwrap();
    let SourceBodyView::Block(block) = body.body().unwrap() else {
        panic!("block")
    };
    let ast::Item::Agent(agent) = &parsed.value.items[0].item else {
        panic!("agent")
    };
    let ast::AgentBody::Source(original) = &agent.body else {
        panic!("source")
    };
    assert!(std::ptr::eq(block, original));
    assert!(Arc::ptr_eq(body.program(), &parsed));
    assert!(Arc::ptr_eq(&body.source().text, &source.text));
    let text = body.format(BodyFormatOptions::default()).unwrap();
    assert!(text.starts_with("if x > 0 {"), "{text}");
    assert!(text.ends_with("return 0;"));
    assert!(!text.contains("unused"));
    assert!(etas_syntax::BodySyntax::parse(SourceFile::anonymous(text)).is_ok());
}

#[test]
fn expression_and_impl_bodies_preserve_native_payload_forms() {
    let source = SourceFile::new(
        SourceId(92),
        None,
        "flow value(x: i32) -> i32 = x + 1; agent Worker; impl agent Worker { flow run() -> i32 { return 2; } }",
    );
    let parsed = Arc::new(etas_syntax::parse_program(source.clone()));
    let expr = SourceBodySyntax::new(
        source.clone(),
        parsed.clone(),
        ProgramBodySelection {
            item: 0,
            impl_member: None,
        },
        BodyParseLimits::default(),
    )
    .unwrap();
    assert!(matches!(
        expr.body().unwrap(),
        SourceBodyView::Expression(_)
    ));
    assert_eq!(expr.format(BodyFormatOptions::default()).unwrap(), "x + 1");
    let method = SourceBodySyntax::new(
        source,
        parsed,
        ProgramBodySelection {
            item: 2,
            impl_member: Some(0),
        },
        BodyParseLimits::default(),
    )
    .unwrap();
    assert_eq!(
        method.format(BodyFormatOptions::default()).unwrap(),
        "return 2;"
    );
}

#[test]
fn retained_source_rejects_foreign_locations_missing_bodies_and_resource_overflow() {
    let source = SourceFile::new(
        SourceId(93),
        None,
        "agent Worker; flow value() -> i32 { return 42; }",
    );
    let parsed = Arc::new(etas_syntax::parse_program(source.clone()));
    assert!(matches!(
        SourceBodySyntax::new(
            source.clone(),
            parsed.clone(),
            ProgramBodySelection {
                item: 0,
                impl_member: None
            },
            BodyParseLimits::default()
        ),
        Err(RetainedBodyError::MissingBody)
    ));
    let mut foreign = source.clone();
    foreign.id = SourceId(94);
    assert!(matches!(
        SourceBodySyntax::new(
            foreign,
            parsed.clone(),
            ProgramBodySelection {
                item: 1,
                impl_member: None
            },
            BodyParseLimits::default()
        ),
        Err(RetainedBodyError::InvalidProgram)
    ));
    assert!(matches!(
        SourceBodySyntax::new(
            source,
            parsed,
            ProgramBodySelection {
                item: 1,
                impl_member: None
            },
            BodyParseLimits {
                max_tokens: 1,
                ..BodyParseLimits::default()
            }
        ),
        Err(RetainedBodyError::Resource(_))
    ));
}

#[test]
fn retained_flow_includes_its_trailing_handler_in_format_and_relocation() {
    let source = SourceFile::new(
        SourceId(95),
        None,
        "flow value() -> i32 { return 1; } with { Error<i32>.raise(_) => { finish 2; } }",
    );
    let parsed = Arc::new(etas_syntax::parse_program(source.clone()));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let body = SourceBodySyntax::new(
        source,
        parsed,
        ProgramBodySelection {
            item: 0,
            impl_member: None,
        },
        BodyParseLimits::default(),
    )
    .unwrap();
    assert!(body.trailing_handler().is_some());
    let text = body.format(BodyFormatOptions::default()).unwrap();
    assert!(text.starts_with("handle {"), "{text}");
    assert!(text.contains("with {"), "{text}");
    assert!(text.contains("finish 2;"), "{text}");
    assert!(etas_syntax::BodySyntax::parse(SourceFile::anonymous(text)).is_ok());
    let relocated = etas_syntax::RelocatedBodySyntax::new(
        body.body().unwrap(),
        body.trailing_handler(),
        body.source(),
        SourceId(96),
        etas_syntax::BodyRelocationLimits::default(),
    )
    .unwrap();
    assert_eq!(
        relocated.trailing_handler().unwrap().span().source,
        SourceId(96)
    );
}
