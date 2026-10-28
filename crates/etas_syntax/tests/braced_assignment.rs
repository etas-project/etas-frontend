use etas_syntax::{
    SourceFile,
    ast::{Expr, FlowBody, Item, Stmt},
    parse_program,
};

fn initializer(expression: &str) -> Expr {
    let parsed = parse_program(SourceFile::anonymous(format!(
        "flow f() -> unit {{ let value = {expression}; }}"
    )));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let Item::Flow(flow) = &parsed.value.items[0].item else {
        panic!("flow")
    };
    let FlowBody::Block(body) = &flow.body else {
        panic!("body")
    };
    let Stmt::Let(binding) = &body.stmts[0] else {
        panic!("binding")
    };
    binding.value.clone()
}

#[test]
fn assignment_leading_braces_are_blocks_in_handled_and_plain_expressions() {
    for body in [
        "{ text = text + suffix; }",
        "{ text = text + call(1, 2); text }",
        "{ text = text + { let part = [1, 2]; part[0] }; }",
    ] {
        let Expr::Block(block) = initializer(body) else {
            panic!("assignment block")
        };
        assert!(matches!(block.block.stmts[0], Stmt::Assign(_)));
        let Expr::Handle(handle) = initializer(&format!("handle {body} with H")) else {
            panic!("handle")
        };
        let Expr::Block(block) = *handle.body else {
            panic!("handled block")
        };
        assert!(matches!(block.block.stmts[0], Stmt::Assign(_)));
    }
}

#[test]
fn record_fields_and_nested_block_semicolons_remain_record_expressions() {
    for expression in [
        "{ text = value }",
        "{ text = call(1, 2), other = [1, 2], }",
        "{ text = { let item = 1; item }, other }",
        "{ text, other }",
        "Row { text = { value = 1; value }, other = [1, 2] }",
    ] {
        assert!(
            matches!(initializer(expression), Expr::Record(_)),
            "{expression}"
        );
        let Expr::Handle(handle) = initializer(&format!("handle {expression} with H")) else {
            panic!("handle")
        };
        assert!(matches!(*handle.body, Expr::Record(_)), "{expression}");
    }
}

#[test]
fn map_and_handler_literals_keep_their_existing_classification() {
    assert!(matches!(
        initializer("{ key => call(1, 2), other => [1, 2] }"),
        Expr::Map(_)
    ));
    assert!(matches!(
        initializer("handler { E.action(value) => { resume value; } }"),
        Expr::Handler(_)
    ));
}
