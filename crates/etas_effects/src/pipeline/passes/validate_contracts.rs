use super::*;

pub(crate) struct ValidateContractsPass;

impl<'a> Pass<EffectPipelineContext<'a>> for ValidateContractsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.validate_contracts", PassKind::Verify)
            .requires(ArtifactSet::from([EFFECT_REGISTRY, EFFECT_FACTS]))
            .produces(ArtifactSet::one(VALIDATED_EFFECT_FACTS))
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        _manager: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let Some(registry) = context.registry.as_ref() else {
            return context.fail(missing(EffectStage::ValidateContracts, "effect registry"));
        };
        let Some(mut effects) = context.effects.take() else {
            return context.fail(missing(
                EffectStage::ValidateContracts,
                "materialized effect facts",
            ));
        };
        if let Err(error) = crate::validate::EffectContractValidator::validate(
            context.input.hir,
            context.input.types,
            registry,
            context.input.tool_bindings,
            &mut effects,
            context.input.reachable_items,
        ) {
            return context.fail(error);
        }
        context.effects = Some(effects);
        produced(VALIDATED_EFFECT_FACTS)
    }
}
