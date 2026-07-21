use etas_cache::{ArtifactKey, InvalidationReport};
use etas_core::SourceId;
use etas_utils::UnitKey;

use crate::ProjectOutput;
use crate::incremental::{CacheReuseReport, affected_bodies, affected_items, affected_modules};
use crate::{MODULE_UNIT_KIND, ModuleId};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiagnosticDelta {
    pub republish_sources: Vec<SourceId>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectSemanticDelta {
    pub changed_sources: Vec<SourceId>,
    pub affected_modules: Vec<UnitKey>,
    pub affected_items: Vec<UnitKey>,
    pub affected_bodies: Vec<UnitKey>,
    pub invalidated_artifacts: Vec<ArtifactKey>,
    pub diagnostics: DiagnosticDelta,
}

impl ProjectSemanticDelta {
    pub(in crate::session) fn from_check(
        changed_sources: Vec<SourceId>,
        output: &ProjectOutput,
        invalidation: &InvalidationReport,
        _cache: &CacheReuseReport,
    ) -> Self {
        let diagnostic_delta = DiagnosticDelta {
            republish_sources: diagnostic_republish_sources(&changed_sources, output),
        };
        Self {
            changed_sources,
            affected_modules: affected_modules(output),
            affected_items: affected_items(output),
            affected_bodies: affected_bodies(output),
            invalidated_artifacts: invalidation.invalidated.clone(),
            diagnostics: diagnostic_delta,
        }
    }
}

fn diagnostic_republish_sources(
    changed_sources: &[SourceId],
    output: &ProjectOutput,
) -> Vec<SourceId> {
    let mut sources = changed_sources.to_vec();
    sources.extend(
        output
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.primary.span.source),
    );
    sources.extend(affected_module_sources(output));
    sources.sort_by_key(|source| source.0);
    sources.dedup();
    sources
}

fn affected_module_sources(output: &ProjectOutput) -> Vec<SourceId> {
    let Some(modules) = output.modules.as_ref() else {
        return Vec::new();
    };
    output
        .affected_modules
        .as_ref()
        .map(|affected| affected.modules.iter())
        .into_iter()
        .flatten()
        .filter(|unit| unit.kind == MODULE_UNIT_KIND)
        .filter_map(|unit| modules.modules.get(ModuleId(unit.id as u32)))
        .flat_map(|module| module.parts.iter())
        .filter_map(|part| modules.parts.get(*part))
        .map(|part| part.source)
        .collect()
}

#[cfg(test)]
mod tests {
    use etas_core::SourceId;

    use super::diagnostic_republish_sources;
    use crate::ProjectOutput;

    #[test]
    fn diagnostic_delta_includes_changed_sources_without_cache_artifacts() {
        let changed_source = SourceId(7);
        let output = ProjectOutput {
            diagnostics: Vec::new(),
            ..empty_project_output()
        };

        assert_eq!(
            diagnostic_republish_sources(&[changed_source], &output),
            vec![changed_source]
        );
    }

    fn empty_project_output() -> ProjectOutput {
        ProjectOutput {
            checked: None,
            diagnostics: Vec::new(),
            sources: None,
            parsed_sources: Vec::new(),
            reused_parsed_sources: 0,
            modules: None,
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
            type_body_outputs: Default::default(),
            types: None,
            effect_body_outputs: Default::default(),
            effect_pipeline_artifacts: None,
            effects: None,
            entry: None,
            reachability: None,
            runtime_source_requirements: None,
        }
    }
}
