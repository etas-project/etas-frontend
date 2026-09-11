use super::*;

fn copied_origin(source: &str) -> Result<Vec<EffectArgRef>, String> {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(17),
        None,
        source,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let context = etas_hir_analysis::HirAnalysisContext::new(&hir);
    let units =
        crate::infer::unit::collect::EffectUnitCollector::collect_with_context(&hir, &context);
    let provenance =
        MemoryProvenance::analyze(&hir, &types, &etas_std::standard_registry(), &units);
    let expr = hir
        .exprs
        .iter()
        .find_map(|(id, expr)| match expr {
            etas_hir::HirExpr::Path(path)
                if path
                    .segments
                    .last()
                    .is_some_and(|segment| segment.name == "copied") =>
            {
                Some(id)
            }
            _ => None,
        })
        .unwrap();
    provenance.arguments(&hir, &types, expr)
}

#[test]
fn branch_origins_are_unioned_and_identical_origins_deduplicated() {
    for other in ["Items", "Other"] {
        let source = format!(
            r#"
module app.main;
import std.memory.{{prepare_put, Any}};
alias Schema = MemoryRegion<{{ Items: Store<string, string>, Other: Store<string, string> }}>;
let Region = std.memory.region<Schema>(stable_id = "branch-provenance", store = "test");
flow choose(first: Store<string, string>, second: Store<string, string>, flag: bool) -> MemoryWriteIntent<string, string> {{
    if flag {{ return prepare_put(first, "key", "value", Any); }}
    return prepare_put(second, "key", "value", Any);
}}
flow main() -> MemoryWriteIntent<string, string> {{
    let copied = choose(Region.Items, Region.{other}, true);
    return copied;
}}
"#
        );
        let actual = copied_origin(&source).unwrap();
        let expected = if other == "Items" {
            vec!["Items"]
        } else {
            vec!["Items", "Other"]
        };
        assert_eq!(
            actual,
            expected
                .into_iter()
                .map(|store| EffectArgRef::Path(
                    ["app", "main", "Region", store].map(str::to_owned).to_vec()
                ))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn non_resource_values_do_not_acquire_provenance_from_their_shape() {
    let error = copied_origin(
        r#"
module app.main;
flow main() -> string {
    let copied = "app.main.Region.Items";
    return copied;
}
"#,
    )
    .unwrap_err();
    assert!(
        error.contains("checked origin") || error.contains("unresolved"),
        "{error}"
    );
}

#[test]
fn prepared_intent_preserves_store_through_multiple_source_calls() {
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(17),
        None,
        r#"
module app.main;
import std.memory.{prepare_put, Any};
alias Schema = MemoryRegion<{ Items: Store<string, string> }>;
let Region = std.memory.region<Schema>(stable_id = "intent-provenance", store = "test");
flow prepare(store: Store<string, string>) -> MemoryWriteIntent<string, string> {
    return prepare_put(store, "key", "value", Any);
}
flow wrap(store: Store<string, string>) -> MemoryWriteIntent<string, string> { return prepare(store); }
flow main() -> MemoryWriteIntent<string, string> {
    let intent = wrap(Region.Items);
    let copied = intent;
    return copied;
}
"#,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let context = etas_hir_analysis::HirAnalysisContext::new(&hir);
    let units =
        crate::infer::unit::collect::EffectUnitCollector::collect_with_context(&hir, &context);
    let provenance =
        MemoryProvenance::analyze(&hir, &types, &etas_std::standard_registry(), &units);
    let expr = hir
        .exprs
        .iter()
        .find_map(|(id, expr)| match expr {
            etas_hir::HirExpr::Path(path)
                if path
                    .segments
                    .last()
                    .is_some_and(|name| name.name == "copied") =>
            {
                Some(id)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(
        provenance.arguments(&hir, &types, expr).unwrap(),
        vec![EffectArgRef::Path(
            ["app", "main", "Region", "Items"]
                .map(str::to_owned)
                .to_vec()
        )]
    );
}
