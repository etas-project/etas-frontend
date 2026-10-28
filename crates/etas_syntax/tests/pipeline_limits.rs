use etas_syntax::{
    SourceFile,
    ast::{Expr, Item, Stmt},
    parse_program,
};

#[test]
fn a_stage_limit_does_not_consume_the_next_pipeline_stage() {
    let parsed = parse_program(SourceFile::anonymous(
        r#"
flow main(value: i32) -> i32 {
    return value ~> first limit Tokens(23) ~> second limit Tokens(11);
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let Item::Flow(flow) = &parsed.value.items[0].item else {
        panic!("flow");
    };
    let Stmt::Return(returned) = &flow.body.stmts[0] else {
        panic!("return");
    };
    let Some(Expr::Pipeline(pipeline)) = &returned.value else {
        panic!("pipeline");
    };
    assert_eq!(pipeline.stages.len(), 2);
    for stage in &pipeline.stages {
        assert!(matches!(stage.expr.as_ref(), Expr::Path(_)));
        assert_eq!(stage.limits.len(), 1);
        assert!(
            matches!(&stage.limits[0], Expr::Call(_)),
            "{:?}",
            stage.limits
        );
    }
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
