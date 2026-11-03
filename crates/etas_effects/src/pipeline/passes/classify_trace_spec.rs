use super::*;

pub(crate) struct ClassifyTraceSpecPass;

impl<'a> Pass<EffectPipelineContext<'a>> for ClassifyTraceSpecPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.trace_spec.classify", PassKind::Analysis)
            .requires(ArtifactSet::one(TRACE_SPEC_ANALYSIS))
            .produces(ArtifactSet::one(TRACE_STATIC_DISPOSITIONS))
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        manager: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let Some(analysis) = context.trace_spec_analysis.as_ref() else {
            return context.fail(missing(
                EffectStage::AnalyzeTraceSpecs,
                "trace spec analysis",
            ));
        };
        let dispositions = match crate::trace_spec::collect_trace_static_dispositions(
            analysis,
            context.input.reachable_items,
            manager.execution_control(),
        ) {
            Ok(value) => value,
            Err(error) => return context.fail(error),
        };
        context.trace_static_dispositions = Some(dispositions);
        produced(TRACE_STATIC_DISPOSITIONS)
    }
}
