use super::*;

#[test]
fn recursive_type_graphs_preserve_callable_payload_presence_without_expansion() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
        r#"
module app;
enum Tree<T> { Leaf(T), Children(Array<Tree<T>>), }
type Callbacks = { next: (i32) -> i32 ![] };
flow numbers(value: Tree<i32>) -> unit { return; }
flow callbacks(value: Tree<Callbacks>) -> unit { return; }
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::HirRevision::try_new(etas_hir::lower_program(&parsed.value)).unwrap();
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let std_registry = etas_std::standard_registry();
    let registry = crate::EffectRegistry::with_standard_effects_from(&std_registry);
    let semantics = EffectSemantics::new(&hir, &types, &std_registry, &registry, &[], &[]);
    let parameter = |name| {
        hir.items
            .iter()
            .find_map(|(item, declaration)| {
                let etas_hir::HirItem::Flow(flow) = declaration else {
                    return None;
                };
                if hir.symbols.get(flow.symbol)?.name != name {
                    return None;
                }
                let etas_types::ItemSignature::Flow(signature) =
                    types.facts.item_signatures.get(&item)?
                else {
                    return None;
                };
                signature.params.first().copied()
            })
            .unwrap()
    };
    for (name, expected) in [
        ("numbers", false),
        ("callbacks", true),
        ("numbers", false),
        ("callbacks", true),
    ] {
        assert_eq!(
            semantics.type_contains_flow(parameter(name)),
            expected,
            "{name}"
        );
    }
}

#[test]
fn transparent_latent_values_require_registry_identity_not_the_name_unwrap() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(0),
        None,
        r#"
import std.option.unwrap as option_unwrap;
flow unwrap(value: i32) -> i32 { return value; }
flow main() -> unit {
    option_unwrap(Some(1));
    unwrap(1);
    return;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::HirRevision::try_new(etas_hir::lower_program(&parsed.value)).unwrap();
    let types = etas_types::TypeOutput::default();
    let std_registry = etas_std::standard_registry();
    let registry = crate::EffectRegistry::with_standard_effects_from(&std_registry);
    let semantics = EffectSemantics::new(&hir, &types, &std_registry, &registry, &[], &[]);
    for (name, expected) in [("option_unwrap", true), ("unwrap", false)] {
        let callee = hir
            .exprs
            .iter()
            .find_map(|(_, expr)| {
                let HirExpr::Call { callee, .. } = expr else {
                    return None;
                };
                let HirExpr::Path(path) = hir.exprs.get(*callee)? else {
                    return None;
                };
                (path.segments.last()?.name == name).then_some(*callee)
            })
            .unwrap();
        assert_eq!(
            semantics.is_latent_transparent_call(callee),
            expected,
            "{name}"
        );
    }
}
