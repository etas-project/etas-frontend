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
            ]))
            .produces(ArtifactSet::one(VALIDATED_TRACE_SPEC_FACTS))
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        _manager: &mut PassManager<EffectPipelineContext<'a>>,
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
                external_summaries: context.input.external_summaries,
                reachable_items: context.input.reachable_items,
            },
        ) {
            return context.fail(error);
        }
        context.effects = Some(effects);
        produced(VALIDATED_TRACE_SPEC_FACTS)
    }
}
