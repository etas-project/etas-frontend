mod anchors;
mod body_artifacts;
mod external_metadata;

use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use self::anchors::{ExternalImportAnchorIndex, external_artifact_anchors};
use self::body_artifacts::record_reused_effect_body_artifacts;
use self::external_metadata::{
    external_effect_metadata, external_effect_summaries, external_trace_spec_summaries,
    tool_provider_bindings,
};
use crate::incremental::CheckScope;
use crate::passes::artifacts::{
    ANALYSIS_REACHABILITY_FACTS, EFFECT_OUTPUT, HIR_OUTPUT, MODULE_INDEX, PREPARED_EFFECT_REGISTRY,
    PROJECT_ENTRY, RESOLVED_IMPORTS, SIGNATURE_FACTS, TYPE_ENVIRONMENT, TYPE_OUTPUT,
    VALIDATED_EXTERNAL_ENVIRONMENT, global_with_diagnostics,
};
use crate::{ProjectContext, UnitKind, UnitTarget};

pub struct PrepareEffectRegistryPass;

impl Pass<ProjectContext> for PrepareEffectRegistryPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("PrepareEffectRegistryPass", PassKind::Analysis)
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                SIGNATURE_FACTS,
                TYPE_ENVIRONMENT,
                RESOLVED_IMPORTS,
                VALIDATED_EXTERNAL_ENVIRONMENT,
            ]))
            .produces(global_with_diagnostics([PREPARED_EFFECT_REGISTRY]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        let (Some(hir), Some(signatures), Some(type_environment), Some(imports), Some(environment)) = (
            context.hir.as_ref(),
            context.signature_types.as_ref(),
            context.type_environment.as_ref(),
            context.resolved_imports.as_ref(),
            context.validated_external_environment.as_ref(),
        ) else {
            return PassResult::failed(
                "effect registry preparation requires checked signature and import artifacts",
            );
        };
        let environment = environment.environment();
        let anchors = ExternalImportAnchorIndex::build(imports, environment);
        let mut diagnostics = Vec::new();
        let metadata =
            match external_effect_metadata(environment, signatures, &anchors, &mut diagnostics) {
                Ok(metadata) => metadata,
                Err(error) => return PassResult::failed(error),
            };
        let prepared = match etas_effects::PreparedEffectRegistry::build(
            &hir.hir,
            signatures,
            type_environment,
            context.std_registry.clone(),
            std::sync::Arc::new(metadata),
            manager.execution_control(),
        ) {
            Ok(prepared) => prepared,
            Err(error) => return pipeline_failure(error),
        };
        context.diagnostics.extend(diagnostics);
        context.prepared_effect_registry = Some(prepared);
        PassResult::changed(
            PreservedArtifacts::All,
            ArtifactSet::one(PREPARED_EFFECT_REGISTRY),
        )
    }
}

pub struct RunEffectPipelinePass;

impl Pass<ProjectContext> for RunEffectPipelinePass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("RunEffectPipelinePass", PassKind::Transform)
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                MODULE_INDEX,
                TYPE_OUTPUT,
                PREPARED_EFFECT_REGISTRY,
                RESOLVED_IMPORTS,
                VALIDATED_EXTERNAL_ENVIRONMENT,
                PROJECT_ENTRY,
                ANALYSIS_REACHABILITY_FACTS,
            ]))
            .produces(global_with_diagnostics([EFFECT_OUTPUT]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        if let Err(error) = record_reused_effect_body_artifacts(context) {
            return PassResult::failed(error);
        }
        let Some(hir) = context.hir.as_ref() else {
            return PassResult::failed("effect checking requires finalized HIR output");
        };
        let Some(types) = context.types.as_ref() else {
            return PassResult::failed("effect checking requires finalized type output");
        };
        let Some(resolved_imports) = context.resolved_imports.as_ref() else {
            return PassResult::failed(
                "effect checking requires resolved imports for external metadata anchors",
            );
        };
        let Some(environment) = context
            .validated_external_environment
            .as_ref()
            .map(|validated| validated.environment())
        else {
            return PassResult::failed("effect checking requires validated external metadata");
        };
        let external_anchors = ExternalImportAnchorIndex::build(resolved_imports, environment);
        let mut external_metadata_diagnostics = Vec::new();
        let Some(prepared) = context.prepared_effect_registry.take() else {
            return PassResult::failed("effect checking requires the prepared effect registry");
        };
        let external_effect_metadata = prepared.dependencies().clone();
        let external_summaries = external_effect_summaries(
            environment,
            types,
            &external_anchors,
            &mut external_metadata_diagnostics,
        );
        let external_trace_specs = external_trace_spec_summaries(
            environment,
            types,
            &external_anchors,
            &mut external_metadata_diagnostics,
        );
        let external_artifact_anchors = external_artifact_anchors(environment, &external_anchors);
        let tool_bindings = tool_provider_bindings(environment);
        let reachable_items = (context.check_scope == CheckScope::EntryReachable)
            .then_some(context.reachability.as_ref())
            .flatten()
            .map(|reachability| &reachability.analysis_items);
        let frozen_calls = match &context.candidate {
            Some(candidate) => {
                let Some(modules) = context.modules.as_ref() else {
                    return PassResult::failed("frozen callable binding requires the module index");
                };
                let Some(reachable_items) = reachable_items else {
                    return PassResult::failed(
                        "frozen callable binding requires checked candidate reachability",
                    );
                };
                Some(crate::code::definition::FrozenCalls {
                    overlay: &candidate.overlay,
                    hir: &hir.hir,
                    types,
                    imports: resolved_imports,
                    modules,
                    std_registry: context.std_registry.as_ref(),
                    limits: candidate.descriptor_limits,
                    reachable_items,
                })
            }
            None => None,
        };
        let input = etas_effects::EffectPipelineInput {
            frozen_calls: frozen_calls
                .as_ref()
                .map(|provider| provider as &dyn etas_effects::FrozenCallableSummaryProvider),
            hir: &hir.hir,
            types,
            std_registry: context.std_registry.as_ref(),
            dependency_metadata: Some(external_effect_metadata.as_ref()),
            tool_bindings: &tool_bindings,
            external_summaries: &external_summaries,
            external_trace_specs: &external_trace_specs,
            external_artifact_anchors: &external_artifact_anchors,
            reachable_items,
        };
        let result = etas_effects::RunEffectPipeline::run_prepared(
            input,
            prepared,
            manager.execution_control_handle(),
        );
        let pipeline_output = match result {
            Ok(output) => output,
            Err(error) => return pipeline_failure(error),
        };
        let mut output = pipeline_output.effects;
        let Some(entry) = context.entry.as_ref() else {
            return PassResult::failed(
                "effect checking requires the resolved project entry artifact",
            );
        };
        output.facts.interpreter_support.entry = entry
            .resolved
            .as_ref()
            .and_then(|entry| output.facts.interpreter_support.items.get(&entry.item))
            .cloned();
        context.effect_pipeline_artifacts = Some(std::sync::Arc::new(pipeline_output.artifacts));
        context.effect_body_outputs.clear();
        if let (Some(units), Some(bindings)) = (&context.units, &context.hir_body_bindings) {
            let mut body_items = Vec::new();
            for (unit, node) in units.nodes.iter() {
                if node.kind != UnitKind::Body {
                    continue;
                }
                if context.check_scope == CheckScope::EntryReachable
                    && context
                        .reachability
                        .as_ref()
                        .is_some_and(|reachability| !reachability.analysis_bodies.contains(&unit))
                {
                    continue;
                }
                let UnitTarget::AstBody(body) = &node.target else {
                    continue;
                };
                let Some(item) = bindings.body_items.get(body).copied() else {
                    continue;
                };
                body_items.push((unit, item));
            }
            let mut bodies = match etas_effects::BodyEffectOutput::project(
                &output,
                &hir.hir,
                body_items.iter().map(|(_, item)| *item),
            ) {
                Ok(bodies) => bodies,
                Err(error) => return PassResult::failed(error),
            };
            for (unit, item) in body_items {
                let Some(body) = bodies.remove(&item) else {
                    return PassResult::failed(
                        "effect body has duplicate or missing item ownership",
                    );
                };
                context
                    .effect_body_outputs
                    .insert(unit, std::sync::Arc::new(body));
            }
        }
        context.diagnostics.extend(external_metadata_diagnostics);
        context
            .diagnostics
            .extend(output.diagnostics.iter().cloned());
        context.effects = Some(output.into());
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(EFFECT_OUTPUT))
    }
}

fn pipeline_failure(error: etas_effects::EffectPipelineError) -> PassResult {
    match error {
        etas_effects::EffectPipelineError::Interrupted(interruption) => {
            PassResult::interrupted(interruption)
        }
        etas_effects::EffectPipelineError::AnalysisLimitExceeded { .. } => {
            PassResult::interrupted(etas_utils::PipelineInterruption {
                kind: etas_utils::PipelineInterruptionKind::ResourceLimit,
                reason: error.to_string(),
            })
        }
        error => PassResult::failed(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_resource_exhaustion_preserves_its_operational_failure_kind() {
        let result = pipeline_failure(etas_effects::EffectPipelineError::AnalysisLimitExceeded {
            resource: "metadata callable nodes",
            limit: 100,
        });
        let etas_utils::PassControl::Failed(failure) = result.control else {
            panic!("resource limit must stop the pass");
        };
        assert!(matches!(
            failure.interruption,
            Some(etas_utils::PipelineInterruption {
                kind: etas_utils::PipelineInterruptionKind::ResourceLimit,
                ..
            })
        ));
        assert!(failure.message.contains("metadata callable nodes"));
        assert!(result.produced.is_empty());
    }

    #[test]
    fn inconsistent_callable_facts_are_not_reported_as_a_resource_limit() {
        let result = pipeline_failure(etas_effects::EffectPipelineError::InvalidCallableFacts {
            reason: "missing checked graph".into(),
        });
        let etas_utils::PassControl::Failed(failure) = result.control else {
            panic!("missing facts must stop the pass");
        };
        assert!(failure.interruption.is_none());
        assert!(failure.message.contains("missing checked graph"));
        assert!(result.produced.is_empty());
    }
}
