use etas_syntax::{
    SourceFile,
    ast::{Expr, Item, Stmt},
    parse_program,
};

#[test]
fn limit_and_limits_are_ordinary_identifiers_in_pipelines() {
    let parsed = parse_program(SourceFile::anonymous(
        r#"
flow limit(limits: i32) -> i32 {
    return limits ~> first ~> second;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let Item::Flow(flow) = &parsed.value.items[0].item else {
        panic!("flow");
    };
    assert_eq!(flow.name.text, "limit");
    let Stmt::Return(returned) = &flow.body.stmts[0] else {
        panic!("return");
    };
    let Some(Expr::Pipeline(pipeline)) = &returned.value else {
        panic!("pipeline");
    };
    assert_eq!(pipeline.stages.len(), 2);
}

#[test]
fn bounded_program_parsing_preserves_typed_interruption() {
    let source = SourceFile::anonymous("module app; flow main() -> i32 { return 7; }");
    let calls = std::cell::Cell::new(0);
    let result = etas_syntax::parse_program_bounded(&source, Default::default(), &|| {
        let count = calls.get() + 1;
        calls.set(count);
        if count == 3 {
            Err("cancelled while parsing dependency")
        } else {
            Ok(())
        }
    });
    assert!(matches!(
        result,
        Err(etas_syntax::BoundedBodyParseError::Interrupted(
            "cancelled while parsing dependency"
        ))
    ));
    assert_eq!(calls.get(), 3);
}

#[test]
fn bounded_program_parsing_checks_token_resources_before_building_the_ast() {
    let source = SourceFile::anonymous("module app; flow main() -> i32 { return 7; }");
    let result = etas_syntax::parse_program_bounded(
        &source,
        etas_syntax::BodyParseLimits {
            max_tokens: 3,
            ..Default::default()
        },
        &|| Ok::<(), std::convert::Infallible>(()),
    );
    assert!(matches!(
        result,
        Err(etas_syntax::BoundedBodyParseError::Resource(
            etas_syntax::BodyResourceError {
                kind: etas_syntax::BodyResourceKind::Tokens,
                limit: 3,
                ..
            }
        ))
    ));
}
