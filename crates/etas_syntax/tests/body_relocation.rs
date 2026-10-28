use etas_syntax::{
    BodyParseLimits, BodyRelocationLimits, BodySyntax, RelocatedBodyError, RelocatedBodySyntax,
    SourceBodyView, SourceFile, SourceId, ast,
};

#[test]
fn relocation_changes_nested_type_pattern_and_expression_spans_without_mutating_original() {
    let source = SourceFile::new(
        SourceId(1),
        None,
        "let value: Option<i32> = Some(42); return match value { Some(number) => number, None => 0, };",
    );
    let syntax = BodySyntax::parse_bounded(source.clone(), BodyParseLimits::default()).unwrap();
    let body = RelocatedBodySyntax::new(
        SourceBodyView::Block(syntax.block()),
        None,
        &source,
        SourceId(81),
        BodyRelocationLimits::default(),
    )
    .unwrap();
    assert!(std::sync::Arc::ptr_eq(&source.text, &body.source().text));
    let SourceBodyView::Block(block) = body.body() else {
        panic!("block")
    };
    let ast::Stmt::Let(binding) = &block.stmts[0] else {
        panic!("let")
    };
    assert_eq!(binding.span.source, SourceId(81));
    assert_eq!(binding.pattern.span().source, SourceId(81));
    assert_eq!(binding.ty.as_ref().unwrap().span().source, SourceId(81));
    assert_eq!(binding.value.span().source, SourceId(81));
    let ast::Stmt::Return(ret) = &block.stmts[1] else {
        panic!("return")
    };
    let ast::Expr::Match(expr) = ret.value.as_ref().unwrap() else {
        panic!("match")
    };
    for arm in &expr.arms {
        assert_eq!(arm.pattern.span().source, SourceId(81));
    }
    assert_eq!(syntax.block().span.source, SourceId(1));
}

#[test]
fn foreign_nested_source_reference_is_rejected_before_clone() {
    let source = SourceFile::new(SourceId(2), None, "let x: i32 = 1; return x;");
    let mut block = etas_syntax::parse_body_contents(source.clone()).value;
    let ast::Stmt::Let(binding) = &mut block.stmts[0] else {
        panic!("let")
    };
    let ast::TypeExpr::Primitive { span, .. } = binding.ty.as_mut().unwrap() else {
        panic!("primitive")
    };
    span.source = SourceId(999);
    assert_eq!(
        RelocatedBodySyntax::new(
            SourceBodyView::Block(&block),
            None,
            &source,
            SourceId(82),
            BodyRelocationLimits::default()
        )
        .unwrap_err(),
        RelocatedBodyError::InvalidSourceReference
    );
}

#[test]
fn relocation_reports_explicit_work_and_depth_limits_without_fallback() {
    let source = SourceFile::new(SourceId(3), None, "return value.field.more;");
    let syntax = BodySyntax::parse(source.clone()).unwrap();
    for limits in [
        BodyRelocationLimits {
            max_nodes: 0,
            ..BodyRelocationLimits::default()
        },
        BodyRelocationLimits {
            max_depth: 0,
            ..BodyRelocationLimits::default()
        },
        BodyRelocationLimits {
            max_source_bytes: 1,
            ..BodyRelocationLimits::default()
        },
    ] {
        assert!(matches!(
            RelocatedBodySyntax::new(
                SourceBodyView::Block(syntax.block()),
                None,
                &source,
                SourceId(83),
                limits
            ),
            Err(RelocatedBodyError::Resource(_))
        ));
    }
}
