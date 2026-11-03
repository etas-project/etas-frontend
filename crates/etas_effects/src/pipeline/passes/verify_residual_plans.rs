use super::*;

/// Independently re-derives and validates every lowered plan. Only verified
/// plans can discharge an unproven conformance in trace validation.
pub(crate) struct VerifyResidualPlansPass;

impl<'a> Pass<EffectPipelineContext<'a>> for VerifyResidualPlansPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.residual.verify_plans", PassKind::Verify)
            .requires(ArtifactSet::from([
                EFFECT_REGISTRY,
                VALIDATED_EFFECT_FACTS,
                TRACE_SPEC_MODELS,
                TRACE_SPEC_ANALYSIS,
                TRACE_STATIC_DISPOSITIONS,
                RESIDUAL_MONITOR_PLANS,
            ]))
            .produces(ArtifactSet::one(VERIFIED_RESIDUAL_PLANS))
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        manager: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let stage = EffectStage::VerifyResidualPlans;
        let Some(lowered) = context.residual_monitor_plans.take() else {
            return context.fail(missing(stage, "residual monitor plans"));
        };
        let verified = match residual_plan_input(context, stage, manager.execution_control())
            .and_then(|input| crate::residual::verify_residual_plans(input, lowered))
        {
            Ok(value) => value,
            Err(error) => return context.fail(error),
        };
        context.verified_residual_plans = Some(verified);
        produced(VERIFIED_RESIDUAL_PLANS)
    }
}
