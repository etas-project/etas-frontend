use super::*;

/// Lowers eligible unproven trace conformances to immutable semantic plans.
/// The output is unverified and is consumed by `VerifyResidualPlansPass`.
pub(crate) struct LowerResidualMonitorPlansPass;

impl<'a> Pass<EffectPipelineContext<'a>> for LowerResidualMonitorPlansPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.residual.lower_monitor_plans", PassKind::Plan)
            .requires(ArtifactSet::from([
                EFFECT_REGISTRY,
                VALIDATED_EFFECT_FACTS,
                TRACE_SPEC_MODELS,
                TRACE_SPEC_ANALYSIS,
                TRACE_STATIC_DISPOSITIONS,
            ]))
            .produces(ArtifactSet::one(RESIDUAL_MONITOR_PLANS))
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        manager: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let stage = EffectStage::LowerResidualPlans;
        let Some(models) = context.trace_spec_models.as_ref() else {
            return context.fail(missing(stage, "trace spec models"));
        };
        // Clause sites are part of the checked facts consumed by canonical
        // projection; Emit later stores the same value.
        let sites = models.source_clause_sites.clone();
        let Some(effects) = context.effects.as_mut() else {
            return context.fail(missing(stage, "validated effect facts"));
        };
        effects.facts.trace_specs.source_clause_sites = sites;
        let lowered = match residual_plan_input(context, stage, manager.execution_control())
            .and_then(crate::residual::lower_residual_monitor_plans)
        {
            Ok(value) => value,
            Err(error) => return context.fail(error),
        };
        context.residual_monitor_plans = Some(lowered);
        produced(RESIDUAL_MONITOR_PLANS)
    }
}
