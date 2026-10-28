use etas_syntax::{SourceFile, SourceId, ast::*, parse_body_contents};

fn expression(text: &str) -> Expr {
    let parsed = parse_body_contents(SourceFile::new(SourceId(41), None, text));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    *parsed.value.final_expr.expect("expression")
}

#[test]
fn inference_is_contextual_to_perform_not_a_global_keyword() {
    assert!(matches!(expression("infer(prompt)"), Expr::Call(_)));
    assert!(matches!(
        expression("perform Custom.infer(prompt)"),
        Expr::Perform(_)
    ));
    let Expr::Infer(inference) = expression("perform infer<string>(prompt)") else {
        panic!("explicit inference")
    };
    assert_eq!(inference.generic_args.len(), 1);
    assert_eq!(inference.args.len(), 1);
    assert_eq!(inference.span.source, SourceId(41));
    let Expr::Infer(inference) = expression("perform infer(prompt = prompt)") else {
        panic!("context-inferred output")
    };
    assert!(inference.generic_args.is_empty());
    assert!(matches!(inference.args.as_slice(), [Arg::Named { .. }]));
}

#[test]
fn malformed_inference_is_not_recovered_as_a_successful_ordinary_call() {
    for source in ["perform infer<string>(", "perform infer<![>(prompt)"] {
        let parsed = parse_body_contents(SourceFile::new(SourceId(41), None, source));
        assert!(!parsed.diagnostics.is_empty(), "{source}");
    }
}
