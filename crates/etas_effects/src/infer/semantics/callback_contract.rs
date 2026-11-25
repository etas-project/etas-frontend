use etas_hir::{HirArg, HirExpr, HirExprId};
use etas_types::Type;

use super::{engine::EffectSemantics, state::EffectState};

impl EffectSemantics<'_> {
    pub(super) fn validate_callback_arguments(&mut self, call: HirExprId, state: &mut EffectState) {
        let (parameters, arguments) = if let Some(method) =
            self.types.facts.checked_method_calls.get(&call)
        {
            let parameters = &method.signature.params;
            let arguments = match self.checked_method_argument_slots(call) {
                Ok(arguments) => arguments,
                Err(error) => {
                    self.mark_incomplete_at(
                        self.hir.exprs[call].span(&self.hir.blocks),
                        error.to_string(),
                        state,
                    );
                    return;
                }
            };
            (parameters, arguments)
        } else if let Some(HirExpr::Call { callee, args, .. }) = self.hir.exprs.get(call) {
            let parameters = if let Some(fact) = self.types.facts.std_intrinsic_calls.get(&call) {
                &fact.signature.params
            } else if let Some(Type::Function(flow)) = self
                .types
                .facts
                .expr_types
                .get(callee)
                .and_then(|ty| self.types.store.get(*ty))
            {
                &flow.input
            } else {
                return;
            };
            let arguments = args
                .iter()
                .map(|arg| match arg {
                    HirArg::Positional(value) | HirArg::Named { value, .. } => Some(*value),
                })
                .collect::<Vec<_>>();
            (parameters, arguments)
        } else {
            return;
        };

        for (parameter_position, (parameter, argument)) in
            parameters.iter().copied().zip(arguments).enumerate()
        {
            let Some(Type::Function(expected)) = self.types.store.get(parameter) else {
                continue;
            };
            let Some(bound) = expected.effects.as_ref().filter(|row| row.tail.is_none()) else {
                continue;
            };
            if let Some(argument) = argument {
                let Some(Type::Function(actual)) = self
                    .types
                    .facts
                    .expr_types
                    .get(&argument)
                    .and_then(|ty| self.types.store.get(*ty))
                else {
                    continue;
                };
                if actual.effects.is_some() {
                    continue;
                }
            }
            // Context may solve after collection. The shared value analysis also
            // follows aliases and captured/returned callable environments.
            let bound = self.row_from_type_ref(bound);
            let checked = match argument {
                Some(argument) => self.latent_summary_for_expr(state, argument, "callback"),
                None => self.default_argument_binding(call, parameter_position, state).and_then(|binding| {
                    let binding = binding.ok_or(super::specialize::EffectSpecializationError::InvalidCallableParameterInterface)?;
                    self.latent_summary_for_origins(self.trace_argument_origins(&binding, state)?, self.hir.exprs[call].span(&self.hir.blocks), "default callback")
                }),
            };
            let location = argument.unwrap_or(call);
            let summary = match checked {
                Ok(summary) => summary,
                Err(error) => {
                    self.mark_incomplete_at(
                        self.hir.exprs[location].span(&self.hir.blocks),
                        error.to_string(),
                        state,
                    );
                    return;
                }
            };
            if matches!(summary.support, crate::InterpreterSupport::Rejected(_)) {
                self.mark_incomplete_at(
                    self.hir.exprs[location].span(&self.hir.blocks),
                    "callback effect contract requires a complete latent behavior summary",
                    state,
                );
                return;
            }
            let coverage = crate::EffectCoverage {
                registry: self.registry,
                types: &self.types.store,
            };
            if !coverage.row_covers(&bound, &summary.escaping_effects) {
                self.diagnostics.push(etas_core::Diagnostic::effect_check(
                    etas_core::EffectDiagnosticCode::EscapedEffect,
                    self.hir.exprs[location].span(&self.hir.blocks),
                    "callback escaping effects exceed the selected parameter's effect bound",
                ));
                state.summary.support = crate::InterpreterSupport::Rejected(
                    crate::FrontendRejectionReason::EscapedEffect,
                );
            }
        }
    }
}
