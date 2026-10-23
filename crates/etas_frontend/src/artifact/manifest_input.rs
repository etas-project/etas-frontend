// Manifest construction borrows compiler artifacts; it never owns a temporary semantic graph.
pub(crate) struct ArtifactManifestInput<'a> {
    pub(crate) checked: &'a Option<crate::CheckedProject>,
    pub(crate) diagnostics: &'a [etas_core::Diagnostic],
    pub(crate) sources: &'a Option<std::sync::Arc<crate::SourceSet>>,
    pub(crate) parsed_sources: &'a [crate::ParsedSource],
    pub(crate) modules: &'a Option<std::sync::Arc<crate::ModuleIndex>>,
    pub(crate) units: &'a Option<std::sync::Arc<crate::UnitTree>>,
    pub(crate) import_graph: &'a Option<std::sync::Arc<crate::ImportGraph>>,
    pub(crate) module_topo_order: &'a Option<crate::ModuleTopoOrder>,
    pub(crate) affected_modules: &'a Option<crate::AffectedModuleSet>,
    pub(crate) resolved_imports: &'a Option<std::sync::Arc<crate::ResolvedImports>>,
    pub(crate) resolved_paths: &'a Option<std::sync::Arc<crate::ResolvedPaths>>,
    pub(crate) hir_item_bindings: &'a Option<crate::HirItemBindings>,
    pub(crate) hir_body_bindings: &'a Option<crate::HirBodyBindings>,
    pub(crate) hir: &'a Option<crate::HirOutput>,
    pub(crate) signature_types: &'a Option<etas_types::TypeOutput>,
    pub(crate) top_level_lets: &'a Option<crate::TopLevelLetFacts>,
    pub(crate) type_body_outputs:
        &'a std::collections::HashMap<crate::UnitId, std::sync::Arc<etas_types::BodyTypeOutput>>,
    pub(crate) types: &'a Option<etas_types::TypeOutput>,
    pub(crate) effect_body_outputs: &'a std::collections::HashMap<
        crate::UnitId,
        std::sync::Arc<etas_effects::BodyEffectOutput>,
    >,
    pub(crate) effect_pipeline_artifacts:
        &'a Option<std::sync::Arc<etas_effects::EffectPipelineArtifacts>>,
    pub(crate) effects: &'a Option<crate::SharedEffectOutput>,
    pub(crate) entry: &'a Option<crate::ProjectEntryFact>,
    pub(crate) reachability: &'a Option<crate::ReachabilityFacts>,
}

impl<'a> From<&'a crate::ProjectOutput> for ArtifactManifestInput<'a> {
    fn from(output: &'a crate::ProjectOutput) -> Self {
        Self {
            checked: &output.checked,
            diagnostics: &output.diagnostics,
            sources: &output.sources,
            parsed_sources: &output.parsed_sources,
            modules: &output.modules,
            units: &output.units,
            import_graph: &output.import_graph,
            module_topo_order: &output.module_topo_order,
            affected_modules: &output.affected_modules,
            resolved_imports: &output.resolved_imports,
            resolved_paths: &output.resolved_paths,
            hir_item_bindings: &output.hir_item_bindings,
            hir_body_bindings: &output.hir_body_bindings,
            hir: &output.hir,
            signature_types: &output.signature_types,
            top_level_lets: &output.top_level_lets,
            type_body_outputs: &output.type_body_outputs,
            types: &output.types,
            effect_body_outputs: &output.effect_body_outputs,
            effect_pipeline_artifacts: &output.effect_pipeline_artifacts,
            effects: &output.effects,
            entry: &output.entry,
            reachability: &output.reachability,
        }
    }
}

impl<'a> From<&'a crate::ProjectContext> for ArtifactManifestInput<'a> {
    fn from(output: &'a crate::ProjectContext) -> Self {
        Self {
            checked: &output.checked,
            diagnostics: &output.diagnostics,
            sources: &output.sources,
            parsed_sources: &output.parsed_sources,
            modules: &output.modules,
            units: &output.units,
            import_graph: &output.import_graph,
            module_topo_order: &output.module_topo_order,
            affected_modules: &output.affected_modules,
            resolved_imports: &output.resolved_imports,
            resolved_paths: &output.resolved_paths,
            hir_item_bindings: &output.hir_item_bindings,
            hir_body_bindings: &output.hir_body_bindings,
            hir: &output.hir,
            signature_types: &output.signature_types,
            top_level_lets: &output.top_level_lets,
            type_body_outputs: &output.type_body_outputs,
            types: &output.types,
            effect_body_outputs: &output.effect_body_outputs,
            effect_pipeline_artifacts: &output.effect_pipeline_artifacts,
            effects: &output.effects,
            entry: &output.entry,
            reachability: &output.reachability,
        }
    }
}

#[cfg(test)]
mod tests {
    use etas_cache::ProjectRevision;
    use etas_utils::{PassControl, PassManager};

    use super::ArtifactManifestInput;
    use crate::{FrontendArtifactManifest, ProjectContext, ProjectInput, SourceInput};

    #[test]
    fn live_context_and_finished_output_share_manifest_inputs_and_fingerprints() {
        let mut context = ProjectContext::new(ProjectInput::single_source(SourceInput::anonymous(
            "module main; public flow helper(x: i32) -> i32 { return x + 1; } flow main() -> i32 { return helper(2); }",
        )));
        let empty = context.current_artifact_manifest();
        assert!(
            empty
                .records
                .iter()
                .all(|record| record.dependencies.is_empty())
        );
        let mut pipeline = crate::pipeline::project_pipeline();
        let run = PassManager::new().run_pipeline(&mut pipeline, &mut context);
        assert!(
            matches!(run.control, PassControl::Continue),
            "{:?}",
            run.control
        );
        assert!(context.checked.is_some(), "{:?}", context.diagnostics);

        let input = ArtifactManifestInput::from(&context);
        assert!(std::ptr::eq(input.hir, &context.hir));
        assert!(std::ptr::eq(input.types, &context.types));
        assert!(std::ptr::eq(input.effects, &context.effects));
        assert!(std::ptr::eq(input.checked, &context.checked));
        assert!(std::ptr::eq(
            input.type_body_outputs,
            &context.type_body_outputs
        ));
        assert!(std::ptr::eq(
            input.effect_body_outputs,
            &context.effect_body_outputs
        ));
        let during = context.current_artifact_manifest();
        let output = context.into_project_output();
        let after = FrontendArtifactManifest::from_output(ProjectRevision(0), &output);
        assert_eq!(during, after);
        assert_eq!(during.fingerprint(), after.fingerprint());
    }
}
