use super::{engine::EffectSemantics, shared::*};
use etas_utils::PartialOrder;

impl EffectSemantics<'_> {
    pub(crate) fn observe_inferred_callback_arguments(
        &mut self,
        callee: EffectUnit,
        site: &CallSite<EffectUnit>,
        mut state: EffectState,
    ) -> EffectState {
        let Some(params) = self.unit_params(callee) else {
            return state;
        };
        let Some(HirExpr::Call { args, .. } | HirExpr::MethodCall { args, .. }) =
            self.hir.exprs.get(site.call)
        else {
            return state;
        };
        let args = args.clone();
        for (index, symbol) in params.into_iter().enumerate() {
            let Some(ty) = self.symbol_value_type(symbol) else {
                continue;
            };
            if !matches!(self.types.store.get(ty), Some(Type::Function(flow)) if flow.effects.is_none())
            {
                continue;
            }
            let summary = args
                .get(index)
                .and_then(arg_expr)
                .zip(self.hir.symbols.get(symbol))
                .and_then(|(expr, parameter)| {
                    self.latent_summary_for_expr(&state, expr, &parameter.name)
                        .ok()
                        .filter(|summary| {
                            !matches!(summary.support, InterpreterSupport::Rejected(_))
                        })
                });
            match summary {
                Some(summary) => {
                    if let Some(expected) = self.frozen_parameter_effects.get(&symbol)
                        && !summary.less_equal(expected)
                    {
                        self.inputs
                            .unresolved_parameter_calls
                            .entry(callee)
                            .or_default()
                            .insert(site.call);
                        state = self.incomplete_at(
                            site.span,
                            "callback behavior is incompatible with the checked inference used by the frozen callable summary",
                            state,
                        );
                        continue;
                    }
                    match self.inputs.inferred_parameter_effects.entry(symbol) {
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            entry.insert(summary);
                        }
                        std::collections::hash_map::Entry::Occupied(mut entry) => {
                            entry.get_mut().join_branch(&summary);
                        }
                    }
                }
                None => {
                    self.inputs
                        .unresolved_parameter_calls
                        .entry(callee)
                        .or_default()
                        .insert(site.call);
                    state = self.incomplete_at(
                        site.span,
                        "first-class flow call requires a checked latent effect fact; an escaping row does not describe requested actions",
                        state,
                    );
                }
            }
        }
        state
    }

    pub(crate) fn validate_returned_flow_source(
        &mut self,
        site: &CallSite<EffectUnit>,
        mut state: EffectState,
    ) -> EffectState {
        if self
            .flow_type_for_expr(site.call)
            .is_some_and(|flow| flow.effects.is_none())
            && self
                .latent_sources_for_expr(&state, site.call)
                .is_none_or(|sources| {
                    sources
                        .iter()
                        .any(|source| matches!(source, LatentSource::Unavailable { .. }))
                })
        {
            state = self.reject_effect_state(
                site.span,
                "returned first-class flow propagation requires a checked latent return source",
                state,
            );
        }
        state
    }
}
