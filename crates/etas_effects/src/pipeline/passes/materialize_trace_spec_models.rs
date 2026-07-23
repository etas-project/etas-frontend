use super::*;

pub(crate) struct MaterializeTraceSpecModelsPass;

impl<'a> Pass<EffectPipelineContext<'a>> for MaterializeTraceSpecModelsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.trace_spec.materialize_models", PassKind::Analysis)
            .requires(ArtifactSet::from([EFFECT_REGISTRY, VALIDATED_EFFECT_FACTS]))
            .produces(ArtifactSet::one(TRACE_SPEC_MODELS))
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        _manager: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let Some(registry) = context.registry.as_ref() else {
            return context.fail(missing(
                EffectStage::MaterializeTraceSpecs,
                "effect registry",
            ));
        };
        let Some(mut effects) = context.effects.take() else {
            return context.fail(missing(EffectStage::MaterializeTraceSpecs, "effect facts"));
        };
        let models = crate::trace_spec::materialize_trace_spec_models(
            context.input.hir,
            context.input.types,
            registry,
            context.input.external_trace_specs,
            &mut effects,
        );
        match models {
            Ok(models) => context.trace_spec_models = Some(models),
            Err(error) => return context.fail(error),
        }
        context.effects = Some(effects);
        produced(TRACE_SPEC_MODELS)
    }
}
