use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use etas_core::{Diagnostic, DiagnosticCode, EffectDiagnosticCode, Severity};

use crate::{CheckedProgramData, ProjectContext, SourceBundle};

use crate::passes::artifacts::{
    CALLABLE_INSTANTIATIONS, CHECKED_PROJECT, CHECKED_REFLECTION_TARGETS, DEFINITION_SOURCES,
    EFFECT_OUTPUT, GENERIC_EFFECT_BINDINGS, HANDLER_INSTANTIATIONS, HIR_OUTPUT,
    INTERPRETER_SUPPORT, MODULE_INDEX, PROJECT_ENTRY, REACHABILITY_FACTS, RESIDUAL_BACKEND_SUPPORT,
    RESOLVED_IMPORTS, TOP_LEVEL_LET_FACTS, TYPE_OUTPUT, VALIDATED_EXTERNAL_ENVIRONMENT,
    VALIDATED_ITEM_ANNOTATIONS, VERIFIED_RESIDUAL_PLANS,
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
                VERIFIED_RESIDUAL_PLANS,
                RESIDUAL_BACKEND_SUPPORT,
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
            if let Err(reason) = crate::passes::execution::validate_residual_publication(context) {
                return PassResult::failed(reason);
            }
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
            let (Some(residual_plans), Some(residual_support)) =
                (&context.residual_plans, &context.residual_support)
            else {
                return PassResult::failed("checked project requires verified residual artifacts");
            };
            let residual = match crate::residual::CheckedResidualPlans::new(
                residual_plans,
                residual_support,
                &context.input.options.residual_target,
                context.residual_identities.clone(),
                context
                    .candidate
                    .as_ref()
                    .is_some_and(|candidate| candidate.bound_application.is_some()),
                context
                    .candidate
                    .as_ref()
                    .map_or_else(Default::default, |candidate| candidate.descriptor_limits),
            ) {
                Ok(residual) => residual,
                Err(reason) => return PassResult::failed(reason),
            };
            let mut data = CheckedProgramData {
                reflection_targets: reflection_targets.clone(),
                foreign_type_bindings: context
                    .candidate
                    .as_ref()
                    .and_then(|candidate| candidate.bound_application.as_ref())
                    .map(|application| application.foreign_type_bindings.clone())
                    .unwrap_or_default(),
                native_instantiations: instantiations.clone(),
                compiler_version: crate::FRONTEND_COMPILER_VERSION.to_owned(),
                std_registry: context.std_registry.clone(),
                project_environment_fingerprint: definition_environment.fingerprint().to_owned(),
                dependency_metadata_fingerprints: environment
                    .canonical_dependency_metadata_fingerprints(),
                sources: SourceBundle {
                    stable_source_keys: context
                        .sources
                        .as_ref()
                        .map(|sources| sources.stable_source_keys.clone())
                        .unwrap_or_default(),
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
                residual: std::sync::Arc::default(),
            };
            // Ordinary plans are bound to their owners' checked revisions only
            // now that every contract fact has been finalized. A candidate
            // binds to its frozen-contract revision when it is emitted.
            if context.candidate.is_none() && !residual.is_empty() {
                data.residual = std::sync::Arc::new(residual);
                let binding = match data.residual.bind_ordinary(&data) {
                    Ok(binding) => binding,
                    Err(error) => {
                        return PassResult::failed(format!(
                            "residual plans cannot be bound to their checked revisions: {error:?}"
                        ));
                    }
                };
                let Ok(residual) = std::sync::Arc::try_unwrap(std::mem::take(&mut data.residual))
                else {
                    return PassResult::failed("residual plans were shared before binding");
                };
                data.residual = std::sync::Arc::new(residual.with_binding(binding));
            } else {
                data.residual = std::sync::Arc::new(residual);
            }
            context.checked = Some(std::sync::Arc::new(data));
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
