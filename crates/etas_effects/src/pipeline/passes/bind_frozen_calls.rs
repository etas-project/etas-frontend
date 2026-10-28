use super::*;

pub(crate) struct BindFrozenCallsPass;

impl<'a> Pass<EffectPipelineContext<'a>> for BindFrozenCallsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.bind_frozen_calls", PassKind::Analysis)
            .requires(ArtifactSet::one(EFFECT_REGISTRY))
            .produces(ArtifactSet::one(FROZEN_CALL_SUMMARIES))
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _: &PassContext<EffectPipelineContext<'a>>,
        _: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let Some(registry) = context.registry.as_ref() else {
            return context.fail(missing(EffectStage::BindFrozenCalls, "effect registry"));
        };
        let summaries = match context.input.frozen_calls {
            Some(provider) => match provider.project(registry) {
                Ok(summaries) => summaries,
                Err(error) => return context.fail(error),
            },
            None => Default::default(),
        };
        context.frozen_calls = Some(std::sync::Arc::new(summaries));
        produced(FROZEN_CALL_SUMMARIES)
    }
}
