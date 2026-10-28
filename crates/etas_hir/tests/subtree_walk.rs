use std::collections::{BTreeSet, HashSet};

use etas_hir::{
    BodyView, ExprView, HirBodyRef, HirExpr, HirExprId, HirLambdaBody, HirVisitor, lower_program,
    walk_expr, walk_expr_tree,
};

#[derive(Default)]
struct Trace {
    exprs: HashSet<HirExprId>,
    bodies: HashSet<HirBodyRef>,
    paths: BTreeSet<String>,
    stack: Vec<HirBodyRef>,
}

impl<'view, 'hir> HirVisitor<'view, 'hir> for Trace {
    fn enter_expr(&mut self, expr: ExprView<'view, 'hir>) {
        assert!(
            self.exprs.insert(expr.id()),
            "expression visited twice: {:?}",
            expr.id()
        );
        if let HirExpr::Path(path) = expr.data() {
            self.paths.insert(
                path.segments
                    .iter()
                    .map(|segment| segment.name.as_str())
                    .collect::<Vec<_>>()
                    .join("."),
            );
        }
    }

    fn enter_body(&mut self, body: BodyView<'view, 'hir>) {
        assert!(
            self.bodies.insert(body.id()),
            "body visited twice: {:?}",
            body.id()
        );
        self.stack.push(body.id());
    }

    fn exit_body(&mut self, body: BodyView<'view, 'hir>) {
        assert_eq!(self.stack.pop(), Some(body.id()));
    }
}

#[test]
fn expression_tree_includes_owned_bodies_once_without_walking_the_containing_item() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
flow main() -> unit {
    let callback = () => {
        let nested = () => leaf();
        return handle {
            match true { true => first(), false => second(), }
        } with {
            Error<i32>.raise(_) => { finish third(); }
        };
    };
    outside();
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let program = lower_program(&parsed.value);
    let view = etas_hir::HirTreeView::new(&program);
    let root = program
        .exprs
        .iter()
        .find_map(|(expr, data)| {
            matches!(
                data,
                HirExpr::Lambda {
                    body: HirLambdaBody::Block(_),
                    ..
                }
            )
            .then_some(expr)
        })
        .unwrap();
    let mut direct = Trace::default();
    walk_expr(&view, root, &mut direct);
    assert_eq!(direct.exprs.len(), 1);
    assert!(direct.bodies.is_empty());
    let mut tree = Trace::default();
    walk_expr_tree(&view, root, &mut tree);
    assert_eq!(
        tree.paths,
        BTreeSet::from([
            "leaf".into(),
            "first".into(),
            "second".into(),
            "third".into()
        ])
    );
    assert!(tree.stack.is_empty());
    assert!(tree.bodies.len() >= 5, "{:?}", tree.bodies);
}
