use etas_effects::{
    ActionInstanceRef, ActionRef, Effect, EffectPipelineInput, EffectSet, MEMORY_READ_ACTION,
    MEMORY_TAG, MEMORY_WRITE_ACTION, RunEffectPipeline,
};
use etas_types::EffectArgRef;

#[test]
fn intent_commit_and_reconcile_keep_exact_resource_and_storage_error_effects() {
    let source = r#"
module app.main;
import std.memory.{prepare_put, commit, reconcile, operation_ref, Any};
alias Schema = MemoryRegion<{ Items: Store<string, string> }>;
let Region = std.memory.region<Schema>(stable_id = "receipt-effects", store = "test");
flow prepare(store: Store<string, string>) -> MemoryWriteIntent<string, string> {
    return prepare_put(store, "key", "value", Any);
}
flow submit<K, V>(intent: MemoryWriteIntent<K, V>) -> WriteOutcome<MemoryWriteReceipt<K>, MemoryWriteRejection> {
    return commit(intent);
}
flow main() -> unit {
    let intent = prepare(Region.Items);
    let result = submit(intent);
    let found = reconcile(Region.Items, operation_ref(intent));
    return;
}
"#;
    let parsed = etas_syntax::parse_program(etas_core::SourceFile::new(
        etas_core::SourceId(17),
        None,
        source,
    ));
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let hir = etas_hir::lower_program(&parsed.value);
    let types = etas_types::check_program(&hir);
    assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
    let registry = etas_std::standard_registry();
    let output = RunEffectPipeline::run(EffectPipelineInput {
        hir: &hir,
        types: &types,
        std_registry: &registry,
        dependency_metadata: None,
        tool_bindings: &[],
        external_summaries: &[],
        external_trace_specs: &[],
        external_artifact_anchors: &[],
        reachable_items: None,
    })
    .unwrap()
    .effects;
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let item = |name: &str| {
        hir.items
            .iter()
            .find_map(|(id, item)| match item {
                etas_hir::HirItem::Flow(flow)
                    if hir.symbols.get(flow.symbol).unwrap().name == name =>
                {
                    Some(id)
                }
                _ => None,
            })
            .unwrap()
    };
    let summary = &output.facts.item_effects[&item("main")];
    let memory = summary
        .requested_actions
        .effects
        .iter()
        .filter(|effect| match effect {
            Effect::AppliedAction(action) => action.action.tag == MEMORY_TAG,
            Effect::Action(action) => action.tag == MEMORY_TAG,
            Effect::Tag(tag) => *tag == MEMORY_TAG,
            _ => false,
        })
        .cloned()
        .collect::<EffectSet>();
    assert_eq!(
        memory,
        [MEMORY_READ_ACTION, MEMORY_WRITE_ACTION]
            .into_iter()
            .map(|action| {
                Effect::AppliedAction(ActionInstanceRef {
                    action: ActionRef {
                        tag: MEMORY_TAG,
                        action,
                    },
                    args: vec![EffectArgRef::Path(
                        ["app", "main", "Region", "Items"]
                            .map(str::to_owned)
                            .to_vec(),
                    )],
                })
            })
            .collect()
    );
    assert!(
        summary
            .escaping_effects
            .effects
            .iter()
            .any(|effect| match effect {
                Effect::Error(ty) =>
                    matches!(types.store.get(*ty), Some(etas_types::Type::Nominal(nominal))
                if nominal.name == "std.memory.StorageError"),
                _ => false,
            }),
        "{:?}",
        summary.escaping_effects
    );
    assert!(
        output.facts.item_effects[&item("prepare")]
            .requested_actions
            .effects
            .is_empty(),
        "preparation must not request storage I/O"
    );
}
