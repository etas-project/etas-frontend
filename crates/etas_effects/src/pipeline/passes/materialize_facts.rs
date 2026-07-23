use super::*;

pub(crate) struct MaterializeFactsPass;

impl<'a> Pass<EffectPipelineContext<'a>> for MaterializeFactsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.materialize_facts", PassKind::Analysis)
            .requires(ArtifactSet::from([EFFECT_REGISTRY, EFFECT_SUMMARIES]))
            .produces(ArtifactSet::one(EFFECT_FACTS))
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        _manager: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let Some(registry) = context.registry.as_ref() else {
            return context.fail(missing(EffectStage::MaterializeFacts, "effect registry"));
        };
        let Some(analysis) = context.analysis.take() else {
            return context.fail(missing(
                EffectStage::MaterializeFacts,
                "solved effect summaries",
            ));
        };
        let effects = crate::facts::EffectFactProjector::materialize(
            context.input.hir,
            context.input.types,
            registry,
            analysis,
            context.input.reachable_items,
        );
        match effects {
            Ok(effects) => context.effects = Some(effects),
            Err(error) => return context.fail(error),
        }
        produced(EFFECT_FACTS)
    }
}
