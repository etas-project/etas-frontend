use super::*;

pub(crate) struct AnalyzeTraceSpecMonitorsPass;

impl<'a> Pass<EffectPipelineContext<'a>> for AnalyzeTraceSpecMonitorsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.trace_spec.analyze_monitors", PassKind::Analysis)
            .requires(ArtifactSet::from([
                EFFECT_REGISTRY,
                EFFECT_UNITS,
                VALIDATED_EFFECT_FACTS,
                TRACE_SPEC_MODELS,
            ]))
            .produces(ArtifactSet::one(TRACE_SPEC_ANALYSIS))
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        _manager: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let Some(registry) = context.registry.as_ref() else {
            return context.fail(missing(EffectStage::AnalyzeTraceSpecs, "effect registry"));
        };
        let Some(models) = context.trace_spec_models.as_ref() else {
            return context.fail(missing(EffectStage::AnalyzeTraceSpecs, "trace spec models"));
        };
        let Some(units) = context.units.as_deref() else {
            return context.fail(missing(EffectStage::AnalyzeTraceSpecs, "effect units"));
        };
        let Some(mut effects) = context.effects.take() else {
            return context.fail(missing(EffectStage::AnalyzeTraceSpecs, "effect facts"));
        };
        let analysis = crate::trace_spec::analyze_trace_spec_monitors(
            context.input.hir,
            context.input.types,
            registry,
            &mut effects,
            models,
            units,
            context.analysis_context.clone(),
        );
        match analysis {
            Ok(analysis) => context.trace_spec_analysis = Some(analysis),
            Err(error) => return context.fail(error),
        }
        context.effects = Some(effects);
        produced(TRACE_SPEC_ANALYSIS)
    }
}
