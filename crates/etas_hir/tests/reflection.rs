use etas_core::{SourceFile, SourceId};
use etas_hir::{
    HirDumpOptions, HirExpr, HirReflectTarget, HirTreeView, ResolveResult, dump_hir, lower_program,
};
use etas_syntax::parse_program;

fn lower(text: &str) -> etas_hir::HirProgram {
    let parsed = parse_program(SourceFile::new(SourceId(23), None, text));
    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    lower_program(&parsed.value)
}

#[test]
fn reflection_is_a_resolved_reference_leaf_not_a_call_or_body_copy() {
    let text = "flow target() -> i32 { return 42; } flow main() -> unit { let definition = ^target; return; }";
    let hir = lower(text);
    assert!(hir.diagnostics.is_empty(), "{:?}", hir.diagnostics);
    let (id, reflection) = hir
        .exprs
        .iter()
        .find_map(|(id, expr)| match expr {
            HirExpr::Reflect(reflection) => Some((id, reflection)),
            _ => None,
        })
        .unwrap();
    let HirReflectTarget::Path(path) = &reflection.target else {
        panic!("path")
    };
    let ResolveResult::Resolved(symbol) = path.resolution else {
        panic!("resolved target")
    };
    assert_eq!(hir.symbols.get(symbol).unwrap().name, "target");
    assert!(
        hir.exprs
            .iter()
            .all(|(_, expr)| !matches!(expr, HirExpr::Call { .. } | HirExpr::MethodCall { .. }))
    );
    let view = HirTreeView::try_new(&hir).unwrap();
    assert_eq!(view.expr(id).unwrap().children().count(), 0);
    assert_eq!(
        &text[reflection.span.range.start.to_usize()..reflection.span.range.end.to_usize()],
        "^target"
    );
    let source = hir.source_map.expr_sources.get(&id).unwrap();
    assert_eq!(source.span(), reflection.span);
    assert_eq!(source.span().source, SourceId(23));
    assert!(dump_hir(&hir, HirDumpOptions::default()).contains("Reflect target"));
}

#[test]
fn selected_spec_method_keeps_receiver_spec_arguments_and_method_identity() {
    let hir = lower(
        "spec Stage<T> T => T ![]; agent Worker; flow inspect() -> unit { let d = ^Worker::Stage<i32>.run; return; }",
    );
    assert!(hir.diagnostics.is_empty(), "{:?}", hir.diagnostics);
    HirTreeView::try_new(&hir).unwrap();
    let reflection = hir
        .exprs
        .iter()
        .find_map(|(_, expr)| match expr {
            HirExpr::Reflect(reflection) => Some(reflection),
            _ => None,
        })
        .unwrap();
    let HirReflectTarget::SpecMethod {
        receiver,
        spec,
        spec_args,
        method,
        method_span,
    } = &reflection.target
    else {
        panic!("selection")
    };
    assert!(matches!(receiver.resolution, ResolveResult::Resolved(_)));
    assert!(matches!(spec.resolution, ResolveResult::Resolved(_)));
    assert_eq!(spec_args.len(), 1);
    assert_eq!(method, "run");
    assert_eq!(method_span.source, SourceId(23));
}

#[test]
fn reflection_uses_lexical_bindings_and_does_not_guess_unresolved_targets() {
    let hir = lower(
        "agent Worker; flow inspect(Worker: i32) -> unit { let d = ^Worker; let bad = ^Missing; return; }",
    );
    let reflections = hir
        .exprs
        .iter()
        .filter_map(|(_, expr)| match expr {
            HirExpr::Reflect(reflection) => Some(reflection),
            _ => None,
        })
        .collect::<Vec<_>>();
    let ResolveResult::Resolved(symbol) = reflections[0].target.root_path().resolution else {
        panic!("param binding")
    };
    assert_eq!(
        hir.symbols.get(symbol).unwrap().kind,
        etas_hir::SymbolKind::Param
    );
    assert!(matches!(
        reflections[1].target.root_path().resolution,
        ResolveResult::Unresolved
    ));
    assert_eq!(hir.diagnostics.len(), 1);
}
