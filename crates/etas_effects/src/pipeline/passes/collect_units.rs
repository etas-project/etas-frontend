use super::*;

pub(crate) struct CollectUnitsPass;

impl<'a> Pass<EffectPipelineContext<'a>> for CollectUnitsPass {
    fn descriptor(&self) -> PassDescriptor {
        PassDescriptor::new("effects.collect_units", PassKind::Analysis)
            .produces(ArtifactSet::one(EFFECT_UNITS))
    }

    fn run(
        &mut self,
        context: &mut EffectPipelineContext<'a>,
        _pass_context: &PassContext<EffectPipelineContext<'a>>,
        _manager: &mut PassManager<EffectPipelineContext<'a>>,
    ) -> PassResult {
        let mut units =
            EffectUnitCollector::collect_with_context(context.input.hir, &context.analysis_context);
        if let Some(reachable_items) = context.input.reachable_items {
            units.retain(|unit| effect_unit_owner_is_reachable(unit, reachable_items));
        }
        context.units = Some(units);
        produced(EFFECT_UNITS)
    }
}
