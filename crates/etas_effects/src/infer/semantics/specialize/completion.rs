use super::{EffectSpecializationError, TraceArgumentBindings};
use crate::{
    CallableOutcome, EffectSummary, OutcomeAnalysisGap,
    infer::semantics::{engine::EffectSemantics, shared::*},
};
use etas_types::{Assignable, TypeRelation};

impl EffectSemantics<'_> {
    pub(super) fn solve_callback_completion(
        &self,
        summary: &mut EffectSummary,
        bindings: &TraceArgumentBindings,
        type_bindings: &[(String, TypeId)],
        state: &EffectState,
    ) -> Result<(), EffectSpecializationError> {
        self.solve_callback_completion_with(summary, type_bindings, |target, span| {
            let Some(binding) = bindings.get(&target) else {
                return Ok(None);
            };
            let origins = self.trace_argument_origins(binding, state)?;
            self.latent_summary_for_origins(origins, span, "callback completion")
                .map(Some)
        })
    }

    pub(crate) fn solve_callback_completion_with(
        &self,
        summary: &mut EffectSummary,
        type_bindings: &[(String, TypeId)],
        mut resolve: impl FnMut(
            crate::ParameterCallTarget,
            Span,
        ) -> Result<Option<EffectSummary>, EffectSpecializationError>,
    ) -> Result<(), EffectSpecializationError> {
        let Some(exits) = &mut summary.exits else {
            return Ok(());
        };
        let substitutions = type_bindings
            .iter()
            .cloned()
            .collect::<std::collections::HashMap<_, _>>();
        let obligations = exits
            .gaps
            .iter()
            .filter_map(|gap| {
                matches!(gap, OutcomeAnalysisGap::ParameterCall { .. }).then_some(gap.clone())
            })
            .collect::<Vec<_>>();
        for obligation in obligations {
            let OutcomeAnalysisGap::ParameterCall {
                target,
                output,
                source,
            } = &obligation
            else {
                continue;
            };
            let Some(callback) = resolve(*target, source.span())? else {
                continue;
            };
            let Some(actual) = callback.exits.as_ref().filter(|exits| {
                !exits.exits.is_empty()
                    && exits
                        .gaps
                        .iter()
                        .all(|gap| matches!(gap, OutcomeAnalysisGap::ParameterCall { .. }))
                    && exits
                        .exits
                        .iter()
                        .all(|exit| matches!(exit.outcome, CallableOutcome::Returned { .. }))
            }) else {
                continue;
            };
            if !callback.escaping_effects.is_empty() {
                continue;
            }
            let output = etas_types::substitute_named_params_in_store(
                &self.types.store,
                *output,
                &substitutions,
            )?;
            let relation = TypeRelation::new(&self.types.store);
            if actual.exits.iter().all(|exit| matches!(exit.outcome, CallableOutcome::Returned { ty, .. } if relation.assignable(ty, output).is_ok())) {
                exits.gaps.remove(&obligation);
                exits.gaps.extend(actual.gaps.iter().cloned());
            }
        }
        Ok(())
    }
}
