use super::*;

pub(crate) struct SolveSummariesPass;

impl<'a> Pass<EffectPipelineContext<'a>> for SolveSummariesPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.solve_summaries", PassKind::Analysis)
            .requires(ArtifactSet::from([
                EFFECT_REGISTRY,
                EFFECT_UNITS,
                MEMORY_PROVENANCE,
            ]))
            .produces(ArtifactSet::one(EFFECT_SUMMARIES))
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        _manager: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let Some(registry) = context.registry.as_ref() else {
            return context.fail(missing(EffectStage::SolveSummaries, "effect registry"));
        };
        let Some(units) = context.units.as_deref() else {
            return context.fail(missing(EffectStage::SolveSummaries, "effect units"));
        };
        let Some(memory_provenance) = context.memory_provenance.as_ref() else {
            return context.fail(missing(EffectStage::SolveSummaries, "memory provenance"));
        };
        let analysis = run_effect_analysis(
            EffectAnalysisInput {
                hir: context.input.hir,
                types: context.input.types,
                std_registry: context.input.std_registry,
                memory_provenance: memory_provenance.clone(),
                registry,
                tool_bindings: context.input.tool_bindings,
                external_summaries: context.input.external_summaries,
            },
            units,
            context.analysis_context.clone(),
        );
        match analysis {
            Ok(analysis) => context.analysis = Some(analysis),
            Err(error) => return context.fail(error),
        }
        produced(EFFECT_SUMMARIES)
    }
}
