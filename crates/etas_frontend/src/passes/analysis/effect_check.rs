mod anchors;
mod body_artifacts;
mod external_metadata;

use etas_utils::{
    ArtifactSet, Pass, PassContext, PassDescriptor, PassKind, PassManager, PassResult,
    PreservedArtifacts,
};

use self::anchors::{ExternalImportAnchorIndex, external_artifact_anchors};
use self::body_artifacts::{local_effect_output, record_reused_effect_body_artifacts};
use self::external_metadata::{
    external_effect_metadata, external_effect_summaries, external_trace_spec_summaries,
    tool_provider_bindings,
};
use crate::incremental::CheckScope;
use crate::passes::artifacts::{
    EFFECT_OUTPUT, HIR_OUTPUT, RESOLVED_IMPORTS, TYPE_OUTPUT, VALIDATED_EXTERNAL_ENVIRONMENT,
    global_with_diagnostics,
};
use crate::{ProjectContext, UnitKind, UnitTarget};

pub struct RunEffectPipelinePass;

impl Pass<ProjectContext> for RunEffectPipelinePass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("RunEffectPipelinePass", PassKind::Transform)
            .requires(ArtifactSet::from([
                HIR_OUTPUT,
                TYPE_OUTPUT,
                RESOLVED_IMPORTS,
                VALIDATED_EXTERNAL_ENVIRONMENT,
            ]))
            .produces(global_with_diagnostics([EFFECT_OUTPUT]))
    }

    fn run(
        &mut self,
        context: &mut ProjectContext,
        _pass_context: &PassContext<ProjectContext>,
        _manager: &mut PassManager<ProjectContext>,
    ) -> PassResult {
        record_reused_effect_body_artifacts(context);
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
        let external_anchors = ExternalImportAnchorIndex::build(resolved_imports);
        let Some(environment) = context
            .validated_external_environment
            .as_ref()
            .map(|validated| validated.environment())
        else {
            return PassResult::failed("effect checking requires validated external metadata");
        };
        let mut external_metadata_diagnostics = Vec::new();
        let external_effect_metadata = match external_effect_metadata(
            environment,
            types,
            &external_anchors,
            &mut external_metadata_diagnostics,
        ) {
            Ok(metadata) => metadata,
            Err(error) => return PassResult::failed(error),
        };
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
            .map(|reachability| &reachability.reachable_items);
        let pipeline_output =
            match etas_effects::RunEffectPipeline::run(etas_effects::EffectPipelineInput {
                hir: &hir.hir,
                types,
                std_registry: context.std_registry.as_ref(),
                dependency_metadata: Some(&external_effect_metadata),
                tool_bindings: &tool_bindings,
                external_summaries: &external_summaries,
                external_trace_specs: &external_trace_specs,
                external_artifact_anchors: &external_artifact_anchors,
                reachable_items,
            }) {
                Ok(output) => output,
                Err(error) => return PassResult::failed(error.to_string()),
            };
        let output = pipeline_output.effects;
        context.effect_pipeline_artifacts = Some(pipeline_output.artifacts);
        context.effect_body_outputs.clear();
        if let (Some(units), Some(bindings)) = (&context.units, &context.hir_item_bindings) {
            for (unit, node) in units.nodes.iter() {
                if node.kind != UnitKind::Body {
                    continue;
                }
                if context.check_scope == CheckScope::EntryReachable
                    && context
                        .reachability
                        .as_ref()
                        .is_some_and(|reachability| !reachability.reachable_bodies.contains(&unit))
                {
                    continue;
                }
                let UnitTarget::AstBody(body) = &node.target else {
                    continue;
                };
                let Some(item) = bindings.ast_to_hir.get(&body.item).copied() else {
                    continue;
                };
                context
                    .effect_body_outputs
                    .insert(unit, local_effect_output(output.clone(), &hir.hir, item));
            }
        }
        context.diagnostics.extend(external_metadata_diagnostics);
        context
            .diagnostics
            .extend(output.diagnostics.iter().cloned());
        context.effects = Some(output);
        PassResult::changed(PreservedArtifacts::All, ArtifactSet::one(EFFECT_OUTPUT))
    }
}
