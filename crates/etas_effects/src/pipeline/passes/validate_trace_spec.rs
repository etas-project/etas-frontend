use super::*;

pub(crate) struct ValidateTraceSpecPass;

impl<'a> Pass<EffectPipelineContext<'a>> for ValidateTraceSpecPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.trace_spec.validate", PassKind::Verify)
            .requires(ArtifactSet::from([
                EFFECT_REGISTRY,
                VALIDATED_EFFECT_FACTS,
                TRACE_SPEC_MODELS,
                TRACE_SPEC_ANALYSIS,
                TRACE_STATIC_DISPOSITIONS,
                VERIFIED_RESIDUAL_PLANS,
            ]))
            .produces(ArtifactSet::one(VALIDATED_TRACE_SPEC_FACTS))
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        manager: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let Some(registry) = context.registry.as_ref() else {
            return context.fail(missing(EffectStage::ValidateTraceSpecs, "effect registry"));
        };
        let Some(models) = context.trace_spec_models.as_ref() else {
            return context.fail(missing(
                EffectStage::ValidateTraceSpecs,
                "trace spec models",
            ));
        };
        let Some(analysis) = context.trace_spec_analysis.as_ref() else {
            return context.fail(missing(
                EffectStage::ValidateTraceSpecs,
                "trace spec analysis",
            ));
        };
        let Some(dispositions) = context.trace_static_dispositions.as_ref() else {
            return context.fail(missing(
                EffectStage::ValidateTraceSpecs,
                "trace static dispositions",
            ));
        };
        let Some(residual_plans) = context.verified_residual_plans.as_ref() else {
            return context.fail(missing(
                EffectStage::ValidateTraceSpecs,
                "verified residual plans",
            ));
        };
        let Some(mut effects) = context.effects.take() else {
            return context.fail(missing(EffectStage::ValidateTraceSpecs, "effect facts"));
        };
        if let Err(error) = crate::trace_spec::validate_trace_spec_facts(
            &mut effects,
            crate::trace_spec::TraceSpecValidationInput {
                hir: context.input.hir,
                types: context.input.types,
                registry,
                models,
                analysis,
                dispositions,
                residual_plans,
                external_summaries: context.input.external_summaries,
                external_implementations: context.input.external_implementations,
                reachable_items: context.input.reachable_items,
                control: manager.execution_control(),
            },
        ) {
            return context.fail(error);
        }
        context.effects = Some(effects);
        produced(VALIDATED_TRACE_SPEC_FACTS)
    }
}
