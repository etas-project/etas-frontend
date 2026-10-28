use etas_core::{SourceFile, SourceId};
use etas_hir::{HirExpr, HirTreeView, ResolveResult, SymbolDef, lower_program};

#[test]
fn ergonomic_and_explicit_agent_methods_each_own_exactly_one_body() {
    let parsed = etas_syntax::parse_program(SourceFile::new(
        SourceId(41),
        None,
        r#"
agent First() -> i32 { return 1; }
agent Second;
impl agent Second {
    flow run() -> i32 { return 2; }
    flow helper() -> i32 { return run(); }
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    assert!(hir.diagnostics.is_empty(), "{:?}", hir.diagnostics);
    HirTreeView::try_new(&hir).expect("valid method body ownership");
    assert_eq!(hir.agent_methods.len(), 3);
    for method in hir.agent_methods.values() {
        let SymbolDef::Item { item } = hir.symbols.get(method.symbol).unwrap().def else {
            panic!("method item identity");
        };
        assert_eq!(item, method.body);
        let count = hir
            .modules_arena
            .iter()
            .flat_map(|(_, module)| module.items.iter())
            .filter(|item| **item == method.body)
            .count();
        assert_eq!(
            count, 1,
            "method body must appear once in the module inventory"
        );
    }
}

#[test]
fn inference_uses_one_canonical_action_even_when_a_user_effect_shadows_its_owner_name() {
    let parsed = etas_syntax::parse_program(SourceFile::new(
        SourceId(41),
        None,
        r#"
effect Agentic { action infer(prompt: Prompt) -> string; }
agent First(prompt: Prompt) -> string { return perform infer<string>(prompt); }
agent Second(prompt: Prompt) -> string { return perform infer<string>(prompt); }
flow custom(prompt: Prompt) -> string { return perform Agentic.infer(prompt); }
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = lower_program(&parsed.value);
    assert!(hir.diagnostics.is_empty(), "{:?}", hir.diagnostics);
    let inferred = hir
        .exprs
        .iter()
        .filter_map(|(expr, node)| match node {
            HirExpr::Infer { action, args, .. } => {
                assert_eq!(args.len(), 1);
                assert_eq!(
                    HirTreeView::try_new(&hir)
                        .unwrap()
                        .expr(expr)
                        .unwrap()
                        .children()
                        .count(),
                    1
                );
                let ResolveResult::Resolved(symbol) = action.action_symbol else {
                    panic!("canonical std action");
                };
                Some(symbol)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(inferred.len(), 2);
    assert_eq!(inferred[0], inferred[1]);
    let ResolveResult::Resolved(custom) = hir
        .exprs
        .iter()
        .find_map(|(_, node)| match node {
            HirExpr::Perform { action, .. } => Some(action.action_symbol.clone()),
            _ => None,
        })
        .expect("custom action")
    else {
        panic!("source action");
    };
    assert_ne!(custom, inferred[0]);
    assert!(matches!(
        hir.symbols.get(inferred[0]).unwrap().def,
        SymbolDef::Synthetic { .. }
    ));
}
