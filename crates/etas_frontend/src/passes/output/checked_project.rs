use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use etas_core::{Diagnostic, DiagnosticCode, EffectDiagnosticCode, Severity};

use crate::{CheckedProgramData, ProjectContext, SourceBundle};

use crate::passes::artifacts::{
    CALLABLE_INSTANTIATIONS, CHECKED_PROJECT, CHECKED_REFLECTION_TARGETS, DEFINITION_SOURCES,
    EFFECT_OUTPUT, GENERIC_EFFECT_BINDINGS, HANDLER_INSTANTIATIONS, HIR_OUTPUT,
    INTERPRETER_SUPPORT, MODULE_INDEX, PROJECT_ENTRY, REACHABILITY_FACTS, RESOLVED_IMPORTS,
    TOP_LEVEL_LET_FACTS, TYPE_OUTPUT, VALIDATED_EXTERNAL_ENVIRONMENT, VALIDATED_ITEM_ANNOTATIONS,
};

pub struct BuildCheckedProjectPass;

impl Pass<ProjectContext> for BuildCheckedProjectPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("BuildCheckedProjectPass", PassKind::Emit)
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                MODULE_INDEX,
                RESOLVED_IMPORTS,
                TOP_LEVEL_LET_FACTS,
                TYPE_OUTPUT,
                CHECKED_REFLECTION_TARGETS,
                EFFECT_OUTPUT,
                INTERPRETER_SUPPORT,
                PROJECT_ENTRY,
                REACHABILITY_FACTS,
                VALIDATED_EXTERNAL_ENVIRONMENT,
                DEFINITION_SOURCES,
                VALIDATED_ITEM_ANNOTATIONS,
                CALLABLE_INSTANTIATIONS,
                GENERIC_EFFECT_BINDINGS,
                HANDLER_INSTANTIATIONS,
            ]))
            .produces(ArtifactSet::one(CHECKED_PROJECT))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        context.checked = None;
        if !has_blocking_static_diagnostics(&context.diagnostics) {
            let Some(resolved_imports) = context.resolved_imports.as_ref() else {
                return PassResult::failed(
                    "checked definition environment is missing resolved imports",
                );
            };
            let Some(definition_sources) = context.definition_sources.as_ref() else {
                return PassResult::failed(
                    "checked definition environment is missing its native source archive",
                );
            };
            let Some(hir_item_bindings) = context.hir_item_bindings.as_ref() else {
                return PassResult::failed("checked project is missing AST/HIR item bindings");
            };
            let hir = context.hir.as_ref().expect("HIR output should exist");
            let modules = context.modules.as_ref().expect("module index should exist");
            let top_level_lets = context
                .top_level_lets
                .as_ref()
                .expect("top-level let facts should exist");
            let types = context.types.as_ref().expect("type output should exist");
            let Some(reflection_targets) = &context.reflection_targets else {
                return PassResult::failed(
                    "checked project is missing resolved reflection targets",
                );
            };
            if let Err(reason) = reflection_targets.validate_owner(&types.facts) {
                return PassResult::failed(reason);
            }
            if !types.facts.deferred_handler_instantiations.is_empty() {
                return PassResult::failed(
                    "checked project has unproved handler request instantiations",
                );
            }
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
            let Some(definition_environment) = context.validated_external_environment.as_ref()
            else {
                return PassResult::failed(
                    "checked definition environment is missing validated external metadata",
                );
            };
            let environment = definition_environment.environment();
            let Some(instantiations) = &context.native_instantiations else {
                return PassResult::failed("checked program is missing prepared invocation types");
            };
            context.checked = Some(std::sync::Arc::new(CheckedProgramData {
                reflection_targets: reflection_targets.clone(),
                native_instantiations: instantiations.clone(),
                compiler_version: crate::FRONTEND_COMPILER_VERSION.to_owned(),
                std_registry: context.std_registry.clone(),
                project_environment_fingerprint: definition_environment.fingerprint().to_owned(),
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
                hir_item_bindings: std::sync::Arc::new(hir_item_bindings.clone()),
                resolved_imports: resolved_imports.clone(),
                definition_sources: definition_sources.clone(),
                definition_environment: definition_environment.clone(),
                frozen_agent_call_boundaries: if context.candidate.is_some() {
                    hir.hir
                        .items
                        .iter()
                        .filter_map(|(item, declaration)| {
                            matches!(declaration, etas_hir::HirItem::Agent(_)).then_some(item)
                        })
                        .chain(types.facts.agent_methods.values().map(|method| method.body))
                        .collect()
                } else {
                    Default::default()
                },
                compile_options: context.input.options.clone(),
                hir: hir.hir.clone(),
                top_level_lets: top_level_lets.clone(),
                types: types.facts.clone(),
                type_store: types.store.clone(),
                effects: effects.facts.clone(),
                effect_registry: effect_artifacts.registry.clone(),
                external_tool_schemas: environment
                    .external_public_metadata
                    .iter()
                    .flat_map(|metadata| metadata.tool_schemas.iter().cloned())
                    .collect(),
                entry_fact: entry.clone(),
                entry: entry.resolved.as_ref().map(|entry| entry.item),
                reachability: reachability.clone(),
            }));
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
