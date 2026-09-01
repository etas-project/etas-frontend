use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use etas_core::{Diagnostic, DiagnosticCode, EffectDiagnosticCode, Severity};

use crate::{CheckedProject, ProjectContext, SourceBundle};

use crate::passes::artifacts::{
    CHECKED_PROJECT, EFFECT_OUTPUT, HIR_OUTPUT, INTERPRETER_SUPPORT, MODULE_INDEX, PROJECT_ENTRY,
    REACHABILITY_FACTS, TOP_LEVEL_LET_FACTS, TYPE_OUTPUT, VALIDATED_EXTERNAL_ENVIRONMENT,
};

pub struct BuildCheckedProjectPass;

impl Pass<ProjectContext> for BuildCheckedProjectPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("BuildCheckedProjectPass", PassKind::Emit)
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                MODULE_INDEX,
                TOP_LEVEL_LET_FACTS,
                TYPE_OUTPUT,
                EFFECT_OUTPUT,
                INTERPRETER_SUPPORT,
                PROJECT_ENTRY,
                REACHABILITY_FACTS,
                VALIDATED_EXTERNAL_ENVIRONMENT,
            ]))
            .produces(ArtifactSet::one(CHECKED_PROJECT))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        if !has_blocking_static_diagnostics(&context.diagnostics) {
            let hir = context.hir.as_ref().expect("HIR output should exist");
            let modules = context.modules.as_ref().expect("module index should exist");
            let top_level_lets = context
                .top_level_lets
                .as_ref()
                .expect("top-level let facts should exist");
            let types = context.types.as_ref().expect("type output should exist");
            let effects = context
                .effects
                .as_ref()
                .expect("effect output should exist");
            let effect_artifacts = context
                .effect_pipeline_artifacts
                .as_ref()
                .expect("effect pipeline artifacts should exist");
            let entry = context
                .entry
                .as_ref()
                .expect("project entry fact should exist");
            let reachability = context
                .reachability
                .as_ref()
                .expect("reachability facts should exist");
            let environment = context
                .validated_external_environment
                .as_ref()
                .expect("external environment should be validated before checked output")
                .environment();
            context.checked = Some(CheckedProject {
                compiler_version: crate::FRONTEND_COMPILER_VERSION.to_owned(),
                std_registry: context.std_registry.clone(),
                project_environment_fingerprint: environment.canonical_environment_fingerprint(),
                dependency_metadata_fingerprints: environment
                    .canonical_dependency_metadata_fingerprints(),
                sources: SourceBundle {
                    sources: context
                        .sources
                        .as_ref()
                        .map(|sources| {
                            sources
                                .files
                                .iter()
                                .map(|source| source.to_core_source_file())
                                .collect()
                        })
                        .unwrap_or_default(),
                },
                module_index: modules.clone(),
                hir: hir.hir.clone(),
                symbols: hir.hir.symbols.clone(),
                scopes: hir.hir.scopes.clone(),
                source_map: hir.hir.source_map.clone(),
                top_level_lets: top_level_lets.clone(),
                types: types.facts.clone(),
                type_store: types.store.clone(),
                effects: effects.facts.clone(),
                effect_registry: effect_artifacts.registry.clone(),
                interpreter_support: effects.facts.interpreter_support.clone(),
                external_tool_schemas: environment
                    .external_public_metadata
                    .iter()
                    .flat_map(|metadata| metadata.tool_schemas.iter().cloned())
                    .collect(),
                entry_fact: entry.clone(),
                entry: entry.resolved.as_ref().map(|entry| entry.item),
                reachability: reachability.clone(),
            });
        }
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(CHECKED_PROJECT))
    }
}

fn has_blocking_static_diagnostics(diagnostics: &[Diagnostic]) -> bool {
    diagnostics.iter().any(|diagnostic| {
        diagnostic.severity == Severity::Error
            && !matches!(
                diagnostic.code,
                DiagnosticCode::Effect(EffectDiagnosticCode::RuntimeRequiredInPhase1)
            )
    })
}
