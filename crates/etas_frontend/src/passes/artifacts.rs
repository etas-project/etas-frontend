use etas_utils::{ArtifactKey, ArtifactRef, ArtifactSet, UnitKindKey};

pub(crate) const SOURCE_SET: ArtifactKey = ArtifactKey::new("frontend", "source_set");
pub(crate) const PARSED_SOURCE_SET: ArtifactKey = ArtifactKey::new("frontend", "parsed_source_set");
pub(crate) const MODULE_INDEX: ArtifactKey = ArtifactKey::new("frontend", "module_index");
pub(crate) const MODULE_CATALOG: ArtifactKey = ArtifactKey::new("frontend", "module_catalog");
pub(crate) const UNIT_TREE: ArtifactKey = ArtifactKey::new("frontend", "unit_tree");
pub(crate) const IMPORT_GRAPH: ArtifactKey = ArtifactKey::new("frontend", "import_graph");
pub(crate) const MODULE_TOPO_ORDER: ArtifactKey = ArtifactKey::new("frontend", "module_topo_order");
pub(crate) const AFFECTED_MODULE_SET: ArtifactKey =
    ArtifactKey::new("frontend", "affected_module_set");
pub(crate) const PREDECLARED_PROJECT_SYMBOLS: ArtifactKey =
    ArtifactKey::new("frontend", "predeclared_project_symbols");
pub(crate) const NORMALIZED_MODULE_IMPORTS: ArtifactKey =
    ArtifactKey::new("frontend", "normalized_module_imports");
pub(crate) const LOWERED_MODULE_ITEMS: ArtifactKey =
    ArtifactKey::new("frontend", "lowered_module_items");
pub(crate) const HIR_OUTPUT: ArtifactKey = ArtifactKey::new("frontend", "hir");
pub(crate) const RESOLVED_IMPORTS: ArtifactKey = ArtifactKey::new("frontend", "resolved_imports");
pub(crate) const RESOLVED_PATHS: ArtifactKey = ArtifactKey::new("frontend", "resolved_paths");
pub(crate) const SIGNATURE_FACTS: ArtifactKey = ArtifactKey::new("frontend", "signature_facts");
pub(crate) const TOP_LEVEL_LET_FACTS: ArtifactKey =
    ArtifactKey::new("frontend", "top_level_let_facts");
pub(crate) const TYPE_OUTPUT: ArtifactKey = ArtifactKey::new("frontend", "type_facts");
pub(crate) const EFFECT_OUTPUT: ArtifactKey = ArtifactKey::new("frontend", "effect_facts");
pub(crate) const INTERPRETER_SUPPORT: ArtifactKey =
    ArtifactKey::new("frontend", "interpreter_support");
pub(crate) const PROJECT_ENTRY: ArtifactKey = ArtifactKey::new("frontend", "project_entry");
pub(crate) const REACHABILITY_FACTS: ArtifactKey =
    ArtifactKey::new("frontend", "reachability_facts");
pub(crate) const RUNTIME_SOURCE_REQUIREMENTS: ArtifactKey =
    ArtifactKey::new("frontend", "runtime_source_requirements");
pub(crate) const CHECKED_PROJECT: ArtifactKey = ArtifactKey::new("frontend", "checked_project");
pub(crate) const DIAGNOSTICS: ArtifactKey = ArtifactKey::new("frontend", "diagnostics");

pub(crate) fn global_with_diagnostics<const N: usize>(keys: [ArtifactKey; N]) -> ArtifactSet {
    let mut artifacts = ArtifactSet::from(keys);
    artifacts.insert(DIAGNOSTICS);
    artifacts
}

pub(crate) fn unit_kind_with_diagnostics(key: ArtifactKey, unit_kind: UnitKindKey) -> ArtifactSet {
    ArtifactSet::from_iter([
        ArtifactRef::unit_kind(key, unit_kind),
        ArtifactRef::unit_kind(DIAGNOSTICS, unit_kind),
    ])
}
