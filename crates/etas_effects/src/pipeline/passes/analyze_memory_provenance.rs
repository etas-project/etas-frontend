use super::*;

pub(crate) struct AnalyzeMemoryProvenancePass;

impl<'a> Pass<EffectPipelineContext<'a>> for AnalyzeMemoryProvenancePass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.analyze_memory_provenance", PassKind::Analysis)
            .requires(ArtifactSet::one(EFFECT_UNITS))
            .produces(ArtifactSet::one(MEMORY_PROVENANCE))
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        _manager: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let Some(units) = context.units.as_deref() else {
            return context.fail(missing(
                EffectStage::AnalyzeMemoryProvenance,
                "effect units",
            ));
        };
        context.memory_provenance = Some(std::sync::Arc::new(
            crate::infer::memory_provenance::MemoryProvenance::analyze(
                context.input.hir,
                context.input.types,
                context.input.std_registry,
                units,
            ),
        ));
        produced(MEMORY_PROVENANCE)
    }
}
