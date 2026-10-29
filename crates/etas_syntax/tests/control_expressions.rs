use etas_syntax::{
    SourceFile,
    ast::{Expr, FlowBody, Item, Stmt},
    parse_program,
};

fn body(source: &str) -> etas_syntax::ast::Block {
    let parsed = parse_program(SourceFile::anonymous(format!(
        "flow main() -> unit {{ {source} }}"
    )));
    assert!(
        parsed.diagnostics.is_empty(),
        "{source}: {:?}",
        parsed.diagnostics
    );
    let Item::Flow(flow) = &parsed.value.items[0].item else {
        panic!("flow")
    };
    let FlowBody::Block(body) = &flow.body else {
        panic!("block")
    };
    body.clone()
}

#[test]
fn nested_records_are_accepted_inside_delimited_conditions() {
    for condition in [
        "!exercise(true, Session { value = 1 }, false)",
        "exercise(value = Session { value = nested(Session { value = 1 }) })",
        "(Session { value = 1 }).value == 1",
        "[Session { value = 1 }][0].value == 1",
    ] {
        let block = body(&format!("if {condition} {{ return; }}"));
        assert_eq!(block.stmts.len(), 1);
        assert!(matches!(block.stmts[0], Stmt::If(_)));
    }
    let block = body("if flag { return; }");
    let Stmt::If(statement) = &block.stmts[0] else {
        panic!("if")
    };
    assert!(matches!(statement.condition, Expr::Path(_)));
}

#[test]
fn unsemicolonized_tail_if_and_match_are_block_values() {
    for source in [
        "if flag { 1 } else { 2 }",
        "if first { 1 } else if second { 2 } else { 3 }",
        "match flag { true => 1, false => 2 }",
        "if flag { 1 } else { match other { true => 2, false => 3 } }",
    ] {
        let block = body(source);
        assert!(block.stmts.is_empty(), "{source}: {:?}", block.stmts);
        assert!(
            matches!(
                block.final_expr.as_deref(),
                Some(Expr::If(_) | Expr::Match(_))
            ),
            "{source}"
        );
    }
}

#[test]
fn semicolonized_and_non_tail_control_forms_remain_statements() {
    for source in [
        "if flag { 1 } else { 2 };",
        "match flag { true => 1, false => 2 };",
        "if flag { return; }",
        "if first { 1 } else if second { 2 }",
        "if flag { 1 } else { 2 } return;",
        "match flag { true => 1, false => 2 } return;",
    ] {
        let block = body(source);
        assert!(block.final_expr.is_none(), "{source}");
        assert!(
            matches!(block.stmts[0], Stmt::If(_) | Stmt::Match(_)),
            "{source}"
        );
    }
}
