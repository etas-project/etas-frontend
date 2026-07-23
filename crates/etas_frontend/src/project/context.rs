use std::{collections::HashMap, sync::Arc};

use etas_cache::{ArtifactKey as CacheArtifactKey, ProjectRevision};
use etas_core::{Diagnostic, SourceId};
use etas_effects::{EffectOutput, EffectPipelineArtifacts};
use etas_types::TypeOutput;
use etas_utils::{UnitFilterKey, UnitKey, UnitKindKey, UnitOrder, UnitProvider, UnitSelector};

use crate::artifact::FrontendArtifactManifest;
use crate::incremental::BodyArtifactReuseInput;
use crate::incremental::CheckScope;
use crate::output::TopLevelLetFacts;
use crate::passes::artifacts::{EFFECT_OUTPUT, TYPE_OUTPUT};
use crate::{
    AffectedModuleSet, BLOCK_UNIT_KIND, BODY_UNIT_KIND, CheckedProject, EXPRESSION_UNIT_KIND,
    HirBodyBindings, HirItemBindings, HirOutput, ITEM_UNIT_KIND, MODULE_PART_UNIT_KIND,
    MODULE_UNIT_KIND, ModuleCatalog, ModuleIndex, ModulePartId, ModuleTopoOrder, PROJECT_UNIT_KIND,
    ProjectEntryFact, ProjectInput, ReachabilityFacts, ResolvedImports, ResolvedPaths,
    RuntimeSourceRequirements, SOURCE_FILE_UNIT_KIND, SourceFile, SourceSet, UnitId, UnitKind,
    UnitNode, UnitTarget, UnitTree,
};

pub const ENTRY_REACHABLE_UNIT_FILTER: UnitFilterKey =
    UnitFilterKey::new("frontend", "entry_reachable");

pub struct ProjectContext {
    pub input: ProjectInput,
    pub std_registry: Arc<etas_std::StdRegistry>,
    pub incremental: bool,
    pub check_scope: CheckScope,
    pub project_wide_change: bool,
    pub changed_sources: Vec<SourceId>,
    pub(crate) body_artifact_reuse: BodyArtifactReuseInput,
    pub(crate) parsed_source_reuse: HashMap<SourceId, crate::ParsedSource>,
    pub(crate) reused_cache_artifacts: Vec<CacheArtifactKey>,
    pub sources: Option<SourceSet>,
    pub parsed_sources: Vec<crate::ParsedSource>,
    pub reused_parsed_sources: usize,
    pub modules: Option<ModuleIndex>,
    pub module_catalog: Option<ModuleCatalog>,
    pub units: Option<UnitTree>,
    pub import_graph: Option<crate::ImportGraph>,
    pub module_topo_order: Option<ModuleTopoOrder>,
    pub affected_modules: Option<AffectedModuleSet>,
    pub resolved_imports: Option<ResolvedImports>,
    pub resolved_paths: Option<ResolvedPaths>,
    pub hir_item_bindings: Option<HirItemBindings>,
    pub hir_body_bindings: Option<HirBodyBindings>,
    pub hir: Option<HirOutput>,
    pub(crate) signature_types: Option<TypeOutput>,
    pub top_level_lets: Option<TopLevelLetFacts>,
    pub(crate) type_body_outputs: HashMap<UnitId, TypeOutput>,
    pub(crate) effect_body_outputs: HashMap<UnitId, EffectOutput>,
    pub(crate) effect_pipeline_artifacts: Option<EffectPipelineArtifacts>,
    pub types: Option<TypeOutput>,
    pub effects: Option<EffectOutput>,
    pub entry: Option<ProjectEntryFact>,
    pub reachability: Option<ReachabilityFacts>,
    pub runtime_source_requirements: Option<RuntimeSourceRequirements>,
    pub checked: Option<CheckedProject>,
    pub diagnostics: Vec<Diagnostic>,
    pub(crate) hir_lowering: Option<ProjectHirLoweringState>,
}

impl ProjectContext {
    #[cfg(test)]
    pub fn new(input: ProjectInput) -> Self {
        Self::new_with_std_registry(input, Arc::new(etas_std::standard_registry()))
    }

    pub(crate) fn new_with_std_registry(
        input: ProjectInput,
        std_registry: Arc<etas_std::StdRegistry>,
    ) -> Self {
        Self {
            input,
            std_registry,
            incremental: false,
            check_scope: CheckScope::FullProject,
            project_wide_change: false,
            changed_sources: Vec::new(),
            body_artifact_reuse: BodyArtifactReuseInput::default(),
            parsed_source_reuse: HashMap::new(),
            reused_cache_artifacts: Vec::new(),
            sources: None,
            parsed_sources: Vec::new(),
            reused_parsed_sources: 0,
            modules: None,
            module_catalog: None,
            units: None,
            import_graph: None,
            module_topo_order: None,
            affected_modules: None,
            resolved_imports: None,
            resolved_paths: None,
            hir_item_bindings: None,
            hir_body_bindings: None,
            hir: None,
            signature_types: None,
            top_level_lets: None,
            type_body_outputs: HashMap::new(),
            effect_body_outputs: HashMap::new(),
            effect_pipeline_artifacts: None,
            types: None,
            effects: None,
            entry: None,
            reachability: None,
            runtime_source_requirements: None,
            checked: None,
            diagnostics: Vec::new(),
            hir_lowering: None,
        }
    }

    #[cfg(test)]
    pub fn new_incremental(
        input: ProjectInput,
        changed_sources: Vec<SourceId>,
        project_wide_change: bool,
    ) -> Self {
        let mut context = Self::new(input);
        context.incremental = true;
        context.project_wide_change = project_wide_change;
        context.changed_sources = changed_sources;
        context
    }

    #[cfg(test)]
    pub(crate) fn new_incremental_with_reuse(
        input: ProjectInput,
        changed_sources: Vec<SourceId>,
        project_wide_change: bool,
        body_artifact_reuse: BodyArtifactReuseInput,
        parsed_source_reuse: HashMap<SourceId, crate::ParsedSource>,
    ) -> Self {
        let mut context = Self::new_incremental(input, changed_sources, project_wide_change);
        context.body_artifact_reuse = body_artifact_reuse;
        context.parsed_source_reuse = parsed_source_reuse;
        context
    }

    pub(crate) fn new_incremental_with_reuse_and_std_registry(
        input: ProjectInput,
        changed_sources: Vec<SourceId>,
        project_wide_change: bool,
        body_artifact_reuse: BodyArtifactReuseInput,
        parsed_source_reuse: HashMap<SourceId, crate::ParsedSource>,
        std_registry: Arc<etas_std::StdRegistry>,
    ) -> Self {
        let mut context = Self::new_with_std_registry(input, std_registry);
        context.incremental = true;
        context.project_wide_change = project_wide_change;
        context.changed_sources = changed_sources;
        context.body_artifact_reuse = body_artifact_reuse;
        context.parsed_source_reuse = parsed_source_reuse;
        context
    }

    pub(crate) fn with_check_scope(mut self, check_scope: CheckScope) -> Self {
        self.check_scope = check_scope;
        self
    }

    pub fn into_project_output(self) -> crate::ProjectOutput {
        crate::ProjectOutput {
            checked: self.checked,
            diagnostics: self.diagnostics,
            sources: self.sources,
            parsed_sources: self.parsed_sources,
            reused_parsed_sources: self.reused_parsed_sources,
            modules: self.modules,
            units: self.units,
            import_graph: self.import_graph,
            module_topo_order: self.module_topo_order,
            affected_modules: self.affected_modules,
            resolved_imports: self.resolved_imports,
            resolved_paths: self.resolved_paths,
            hir_item_bindings: self.hir_item_bindings,
            hir_body_bindings: self.hir_body_bindings,
            hir: self.hir,
            signature_types: self.signature_types,
            top_level_lets: self.top_level_lets,
            type_body_outputs: self.type_body_outputs,
            types: self.types,
            effect_body_outputs: self.effect_body_outputs,
            effect_pipeline_artifacts: self.effect_pipeline_artifacts,
            effects: self.effects,
            entry: self.entry,
            reachability: self.reachability,
            runtime_source_requirements: self.runtime_source_requirements,
        }
    }

    pub(crate) fn current_artifact_manifest(&self) -> FrontendArtifactManifest {
        FrontendArtifactManifest::from_output(
            ProjectRevision(0),
            &crate::ProjectOutput {
                checked: self.checked.clone(),
                diagnostics: self.diagnostics.clone(),
                sources: self.sources.clone(),
                parsed_sources: self.parsed_sources.clone(),
                reused_parsed_sources: self.reused_parsed_sources,
                modules: self.modules.clone(),
                units: self.units.clone(),
                import_graph: self.import_graph.clone(),
                module_topo_order: self.module_topo_order.clone(),
                affected_modules: self.affected_modules.clone(),
                resolved_imports: self.resolved_imports.clone(),
                resolved_paths: self.resolved_paths.clone(),
                hir_item_bindings: self.hir_item_bindings.clone(),
                hir_body_bindings: self.hir_body_bindings.clone(),
                hir: self.hir.clone(),
                signature_types: self.signature_types.clone(),
                top_level_lets: self.top_level_lets.clone(),
                type_body_outputs: self.type_body_outputs.clone(),
                types: self.types.clone(),
                effect_body_outputs: self.effect_body_outputs.clone(),
                effect_pipeline_artifacts: self.effect_pipeline_artifacts.clone(),
                effects: self.effects.clone(),
                entry: self.entry.clone(),
                reachability: self.reachability.clone(),
                runtime_source_requirements: self.runtime_source_requirements.clone(),
            },
        )
    }

    pub fn source_file_for_unit(&self, unit: UnitKey) -> Option<&SourceFile> {
        if unit.kind != SOURCE_FILE_UNIT_KIND {
            return None;
        }
        self.sources.as_ref().and_then(|sources| {
            sources
                .files
                .iter()
                .find(|source| source.id.0 as u64 == unit.id)
        })
    }

    fn affected_units(
        &self,
        kind: UnitKindKey,
        artifact: Option<etas_utils::ArtifactKey>,
        filter: Option<UnitFilterKey>,
        order: UnitOrder,
    ) -> Vec<UnitKey> {
        let mut units = self
            .affected_modules
            .as_ref()
            .map(|affected| {
                if kind == MODULE_UNIT_KIND {
                    affected.modules.clone()
                } else if kind == MODULE_PART_UNIT_KIND {
                    affected.module_parts.clone()
                } else if kind == ITEM_UNIT_KIND {
                    affected.items.clone()
                } else if kind == BODY_UNIT_KIND {
                    affected.bodies.clone()
                } else {
                    Vec::new()
                }
            })
            .unwrap_or_default();
        match order {
            UnitOrder::ReverseDependencyOrder => units.reverse(),
            UnitOrder::AffectedFirst | UnitOrder::DependencyOrder => {}
            _ => units.sort_by_key(|unit| unit.id),
        }
        if kind == BODY_UNIT_KIND && self.incremental {
            let mut seen = units
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>();
            for body in self.units_by_kind(UnitKind::Body, BODY_UNIT_KIND) {
                let unit = UnitId(body.id as u32);
                if self.body_artifact_missing(unit, artifact) && seen.insert(body) {
                    units.push(body);
                }
            }
            if !matches!(
                order,
                UnitOrder::ReverseDependencyOrder | UnitOrder::AffectedFirst
            ) {
                units.sort_by_key(|unit| unit.id);
            }
        }
        self.apply_unit_filter(kind, filter, units)
    }

    fn apply_unit_filter(
        &self,
        kind: UnitKindKey,
        filter: Option<UnitFilterKey>,
        units: Vec<UnitKey>,
    ) -> Vec<UnitKey> {
        match filter {
            Some(filter) if filter == ENTRY_REACHABLE_UNIT_FILTER => {
                if kind != BODY_UNIT_KIND {
                    panic!("entry-reachable unit filter only supports body units, got {kind:?}");
                }
                if self.check_scope != CheckScope::EntryReachable {
                    return units;
                }
                let reachability = self
                    .reachability
                    .as_ref()
                    .expect("entry-reachable unit filter requires reachability facts");
                units
                    .into_iter()
                    .filter(|unit| {
                        reachability
                            .reachable_bodies
                            .contains(&UnitId(unit.id as u32))
                    })
                    .collect()
            }
            Some(filter) => panic!("unsupported project unit filter {filter:?}"),
            None => units,
        }
    }

    fn body_artifact_missing(
        &self,
        unit: UnitId,
        artifact: Option<etas_utils::ArtifactKey>,
    ) -> bool {
        match artifact {
            Some(artifact) if artifact == TYPE_OUTPUT => {
                !self.type_body_outputs.contains_key(&unit)
            }
            Some(artifact) if artifact == EFFECT_OUTPUT => {
                !self.effect_body_outputs.contains_key(&unit)
            }
            Some(_) | None => false,
        }
    }

    fn units_by_kind(&self, kind: UnitKind, unit_kind: UnitKindKey) -> Vec<UnitKey> {
        self.units
            .as_ref()
            .map(|tree| {
                tree.nodes
                    .iter()
                    .filter_map(|(id, node)| {
                        (node.kind == kind).then_some(UnitKey::new(unit_kind, id.0 as u64))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn module_parts_in_dependency_order(&self, modules: &ModuleIndex) -> Vec<UnitKey> {
        let Some(order) = self.module_topo_order.as_ref() else {
            return modules
                .parts
                .iter()
                .map(|(id, _)| UnitKey::new(MODULE_PART_UNIT_KIND, id.0 as u64))
                .collect();
        };
        order
            .modules
            .iter()
            .filter_map(|module| modules.modules.get(*module))
            .flat_map(|module| module.parts.iter())
            .map(|part| UnitKey::new(MODULE_PART_UNIT_KIND, part.0 as u64))
            .collect()
    }
}

pub(crate) struct ProjectHirLoweringState {
    pub lowering: etas_hir::HirProjectLowering,
    pub module_part_to_module_index: HashMap<ModulePartId, usize>,
    pub normalized_parts: std::collections::HashSet<ModulePartId>,
    pub lowered_parts: std::collections::HashSet<ModulePartId>,
    pub item_bindings: HirItemBindings,
    pub total_parts: usize,
}

impl UnitProvider for ProjectContext {
    fn units(&self, selector: &UnitSelector, order: UnitOrder) -> Vec<UnitKey> {
        if let UnitSelector::Affected(kind) = selector {
            return self.affected_units(*kind, None, None, order);
        }
        if let UnitSelector::AffectedArtifact {
            kind,
            artifact,
            filter,
        } = selector
        {
            return self.affected_units(*kind, Some(*artifact), *filter, order);
        }
        let kind = match selector {
            UnitSelector::Kind(kind) => *kind,
            UnitSelector::Affected(_) | UnitSelector::AffectedArtifact { .. } => {
                unreachable!("affected selector should be handled above")
            }
        };
        let mut units = if kind == SOURCE_FILE_UNIT_KIND {
            self.sources
                .as_ref()
                .map(|sources| {
                    sources
                        .files
                        .iter()
                        .map(|source| UnitKey::new(SOURCE_FILE_UNIT_KIND, source.id.0 as u64))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        } else if kind == MODULE_PART_UNIT_KIND {
            self.modules
                .as_ref()
                .map(|modules| {
                    if order == UnitOrder::DependencyOrder {
                        self.module_parts_in_dependency_order(modules)
                    } else {
                        modules
                            .parts
                            .iter()
                            .map(|(id, _)| UnitKey::new(MODULE_PART_UNIT_KIND, id.0 as u64))
                            .collect::<Vec<_>>()
                    }
                })
                .unwrap_or_default()
        } else if kind == BODY_UNIT_KIND {
            self.units_by_kind(UnitKind::Body, BODY_UNIT_KIND)
        } else if kind == MODULE_UNIT_KIND {
            self.units_by_kind(UnitKind::Module, MODULE_UNIT_KIND)
        } else if kind == ITEM_UNIT_KIND {
            self.units_by_kind(UnitKind::Item, ITEM_UNIT_KIND)
        } else if kind == BLOCK_UNIT_KIND {
            self.units_by_kind(UnitKind::Block, BLOCK_UNIT_KIND)
        } else if kind == EXPRESSION_UNIT_KIND {
            self.units_by_kind(UnitKind::Expression, EXPRESSION_UNIT_KIND)
        } else {
            Vec::new()
        };

        if order == UnitOrder::ReverseDependencyOrder {
            units.reverse();
        }
        units
    }

    fn parent(&self, unit: UnitKey) -> Option<UnitKey> {
        let tree = self.units.as_ref()?;
        let node = unit_node(tree, self.sources.as_ref(), unit)?;
        let parent = node.parent?;
        let parent_node = tree.nodes.get(parent)?;
        Some(unit_key_for_node(parent_node))
    }

    fn children(&self, unit: UnitKey, kind: Option<UnitKindKey>) -> Vec<UnitKey> {
        let Some(tree) = self.units.as_ref() else {
            return Vec::new();
        };
        let Some(node) = unit_node(tree, self.sources.as_ref(), unit) else {
            return Vec::new();
        };
        node.children
            .iter()
            .filter_map(|child| tree.nodes.get(*child))
            .map(unit_key_for_node)
            .filter(|child| kind.is_none_or(|kind| child.kind == kind))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use etas_utils::{PassManager, UnitKey, UnitOrder, UnitProvider, UnitSelector};

    use crate::passes::artifacts::TYPE_OUTPUT;
    use crate::pipeline::project_pipeline;
    use crate::{
        AffectedModuleSet, BODY_UNIT_KIND, ProjectInput, SourceInput, UnitKind,
        project::ProjectContext,
    };

    #[test]
    fn affected_body_selector_does_not_fallback_to_missing_artifacts() {
        let input = ProjectInput::single_source(SourceInput::anonymous(
            r#"
flow helper() -> i64 {
  return 1;
}

flow main() -> i64 {
  return helper();
}
"#,
        ));
        let mut full_context = ProjectContext::new(input.clone());
        let mut pipeline = project_pipeline();
        let mut manager = PassManager::new();
        let run = manager.run_pipeline(&mut pipeline, &mut full_context);
        assert!(matches!(run.control, etas_utils::PassControl::Continue));

        let units = full_context.units.clone().expect("unit tree");
        let mut body_units = units
            .nodes
            .iter()
            .filter_map(|(unit, node)| {
                (node.kind == UnitKind::Body).then_some(UnitKey::new(BODY_UNIT_KIND, unit.0 as u64))
            })
            .collect::<Vec<_>>();
        body_units.sort_by_key(|unit| unit.id);
        assert_eq!(body_units.len(), 2);

        let explicitly_affected = vec![body_units[0]];
        let mut incremental_context = ProjectContext::new(input);
        incremental_context.incremental = true;
        incremental_context.units = Some(units);
        incremental_context.affected_modules = Some(AffectedModuleSet {
            bodies: explicitly_affected.clone(),
            ..AffectedModuleSet::default()
        });

        let affected_bodies =
            incremental_context.units(&UnitSelector::Affected(BODY_UNIT_KIND), UnitOrder::Stable);
        assert_eq!(affected_bodies, explicitly_affected);

        let type_missing_bodies = incremental_context.units(
            &UnitSelector::AffectedArtifact {
                kind: BODY_UNIT_KIND,
                artifact: TYPE_OUTPUT,
                filter: None,
            },
            UnitOrder::Stable,
        );
        assert_eq!(type_missing_bodies, body_units);
    }
}

fn unit_node<'a>(
    tree: &'a UnitTree,
    sources: Option<&SourceSet>,
    unit: UnitKey,
) -> Option<&'a UnitNode> {
    let id = if unit.kind == SOURCE_FILE_UNIT_KIND {
        let source = sources?
            .files
            .iter()
            .find(|source| source.id.0 as u64 == unit.id)?;
        *tree.by_target.get(&UnitTarget::Source(source.id))?
    } else {
        UnitId(unit.id as u32)
    };
    tree.nodes.get(id)
}

fn unit_key_for_node(node: &UnitNode) -> UnitKey {
    let kind = match node.kind {
        UnitKind::Project => PROJECT_UNIT_KIND,
        UnitKind::SourceFile => SOURCE_FILE_UNIT_KIND,
        UnitKind::Module => MODULE_UNIT_KIND,
        UnitKind::ModulePart => MODULE_PART_UNIT_KIND,
        UnitKind::Item => ITEM_UNIT_KIND,
        UnitKind::Body => BODY_UNIT_KIND,
        UnitKind::Block => BLOCK_UNIT_KIND,
        UnitKind::Expression => EXPRESSION_UNIT_KIND,
    };
    let id = match node.target {
        UnitTarget::Source(source) => source.0 as u64,
        _ => node.id.0 as u64,
    };
    UnitKey::new(kind, id)
}
