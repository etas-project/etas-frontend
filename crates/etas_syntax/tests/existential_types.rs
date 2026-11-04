use etas_syntax::{SourceFile, SourceId, ast, parse_program};

#[test]
fn existential_bounds_compose_with_normal_type_positions_and_method_postfixes() {
    let parsed = parse_program(SourceFile::new(
        SourceId(1),
        None,
        r#"module main;
spec Value { flow read(self) -> i32; }
spec Within<R>;
type Root;
flow select(value: ? ~ Value + Within<Root>) -> ? ~ Value { return value; }
flow read(values: Array<? ~ Value>) -> i32 { return values[0].read(); }
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let flow = parsed
        .value
        .items
        .iter()
        .find_map(|item| match &item.item {
            ast::Item::Flow(flow) if flow.name.text == "select" => Some(flow),
            _ => None,
        })
        .unwrap();
    let ast::ParamType::Explicit(ty) = &flow.params[0].ty else {
        panic!("explicit parameter annotation");
    };
    let ast::TypeExpr::Existential { bounds, span } = ty.as_ref() else {
        panic!("existential parameter type");
    };
    assert_eq!(bounds.len(), 2);
    assert_eq!(bounds[0].path.segments[0].text, "Value");
    assert_eq!(bounds[1].path.segments[0].text, "Within");
    assert_eq!(bounds[1].args.len(), 1);
    assert_eq!(span.source, SourceId(1));
    assert!(matches!(
        flow.return_type,
        Some(ast::TypeExpr::Existential { .. })
    ));
}
