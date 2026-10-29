use super::{EffectSpecializationError, TraceArgumentBinding, TraceArgumentBindings};
use crate::{
    CallableOutcome, EffectSummary, OutcomeAnalysisGap,
    infer::semantics::{engine::EffectSemantics, shared::*},
};
use etas_hir::SymbolId;
use etas_types::{Assignable, TypeRelation};

impl EffectSemantics<'_> {
    pub(super) fn forward_callback_outcomes(
        &self,
        summary: &mut EffectSummary,
        bindings: &TraceArgumentBindings,
        params: &[SymbolId],
        state: &EffectState,
    ) -> Result<(), EffectSpecializationError> {
        let Some(exits) = &mut summary.exits else {
            return Ok(());
        };
        let original_gaps = exits.gaps.clone();
        for exit in &mut exits.parameter_exits {
            if let Some(control) = self.query_control {
                control
                    .checkpoint()
                    .map_err(EffectSpecializationError::Interrupted)?;
            }
            let Some(binding @ TraceArgumentBinding::Expression(expression)) =
                bindings.get(&exit.target)
            else {
                continue;
            };
            let origins = self.trace_argument_origins(binding, state)?;
            let mut origins = origins.into_iter();
            let (Some(LatentSource::Parameter { owner, symbol }), None) =
                (origins.next(), origins.next())
            else {
                continue;
            };
            if state.owner != Some(owner) {
                continue;
            }
            let Some(expression) = self.hir.exprs.get(*expression) else {
                continue;
            };
            let old = OutcomeAnalysisGap::ParameterCall {
                target: exit.target,
                output: exit.output,
                source: exit.source.clone(),
            };
            if !original_gaps.contains(&old) {
                continue;
            }
            exits.gaps.remove(&old);
            exit.target = crate::ParameterCallTarget::Native(symbol);
            exit.source = expression.span(&self.hir.blocks).into();
            exit.prefix = self.specialize_parameter_call_trace(&exit.prefix, bindings, state)?;
            if let Some(arguments) = &mut exit.arguments {
                for argument in arguments {
                    if let Some(control) = self.query_control {
                        control
                            .checkpoint()
                            .map_err(EffectSpecializationError::Interrupted)?;
                    }
                    let Some(crate::ParameterArgumentValue::Input { position }) =
                        argument.value.as_ref()
                    else {
                        continue;
                    };
                    argument.value = params
                        .get(*position as usize)
                        .and_then(|symbol| {
                            bindings.get(&crate::ParameterCallTarget::Native(*symbol))
                        })
                        .and_then(|binding| match binding {
                            TraceArgumentBinding::Expression(expr) => self.hir.exprs.get(*expr),
                            TraceArgumentBinding::Origins(_) => None,
                        })
                        .and_then(|expression| {
                            super::super::argument_values::checked_literal_value(
                                self.hir,
                                self.types,
                                expression,
                                argument.ty,
                            )
                            .or_else(|| {
                                state.unit.and_then(|unit| {
                                    self.argument_writes.input_value(
                                        self.hir,
                                        self.types,
                                        unit,
                                        expression,
                                        argument.ty,
                                    )
                                })
                            })
                        });
                }
            }
            exits.gaps.insert(OutcomeAnalysisGap::ParameterCall {
                target: exit.target,
                output: exit.output,
                source: exit.source.clone(),
            });
        }
        Ok(())
    }

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
            if let Some(control) = self.query_control {
                control
                    .checkpoint()
                    .map_err(EffectSpecializationError::Interrupted)?;
            }
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
            let Some(actual) = callback.exits.as_ref().filter(|exits| exits.returns_only()) else {
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
                exits.parameter_exits.retain(|exit| !(exit.target == *target && exit.source == *source));
            }
        }
        Ok(())
    }
}
