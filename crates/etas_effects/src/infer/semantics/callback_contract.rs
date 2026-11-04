use etas_hir::{HirArg, HirExpr, HirExprId};
use etas_types::Type;

use super::{engine::EffectSemantics, state::EffectState};

impl EffectSemantics<'_> {
    pub(super) fn validate_callback_arguments(
        &mut self,
        call: HirExprId,
        mut state: EffectState,
    ) -> EffectState {
        let (parameters, arguments) = if let Some(method) =
            self.types.facts.checked_method_calls.get(&call)
        {
            let parameters = method.signature.params.clone();
            let arguments = match self.checked_method_arguments(call) {
                Ok(arguments) => arguments,
                Err(error) => {
                    return self.incomplete_at(
                        self.hir.exprs[call].span(&self.hir.blocks),
                        error.to_string(),
                        state,
                    );
                }
            };
            (parameters, arguments)
        } else if let Some(HirExpr::Call { callee, args, .. }) = self.hir.exprs.get(call) {
            let parameters = if let Some(fact) = self.types.facts.std_intrinsic_calls.get(&call) {
                fact.signature.params.clone()
            } else if let Some(Type::Function(flow)) = self
                .types
                .facts
                .expr_types
                .get(callee)
                .and_then(|ty| self.types.store.get(*ty))
            {
                flow.input.clone()
            } else {
                return state;
            };
            let arguments = args
                .iter()
                .map(|arg| match arg {
                    HirArg::Positional(value) | HirArg::Named { value, .. } => *value,
                })
                .collect::<Vec<_>>();
            (parameters, arguments)
        } else {
            return state;
        };

        for (parameter, argument) in parameters.into_iter().zip(arguments) {
            let Some(Type::Function(expected)) = self.types.store.get(parameter) else {
                continue;
            };
            let Some(bound) = expected.effects.as_ref().filter(|row| row.tail.is_none()) else {
                continue;
            };
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
            // Context may solve after collection. The shared value analysis also
            // follows aliases and captured/returned callable environments.
            let bound = self.row_from_type_ref(bound);
            let summary = match self.latent_summary_for_expr(&state, argument, "callback") {
                Ok(summary) => summary,
                Err(error) => {
                    return self.incomplete_at(
                        self.hir.exprs[argument].span(&self.hir.blocks),
                        error.to_string(),
                        state,
                    );
                }
            };
            if matches!(summary.support, crate::InterpreterSupport::Rejected(_)) {
                return self.incomplete_at(
                    self.hir.exprs[argument].span(&self.hir.blocks),
                    "callback effect contract requires a complete latent behavior summary",
                    state,
                );
            }
            let coverage = crate::EffectCoverage {
                registry: self.registry,
                types: &self.types.store,
            };
            if !coverage.row_covers(&bound, &summary.escaping_effects) {
                self.diagnostics.push(etas_core::Diagnostic::effect_check(
                    etas_core::EffectDiagnosticCode::EscapedEffect,
                    self.hir.exprs[argument].span(&self.hir.blocks),
                    "callback escaping effects exceed the selected parameter's effect bound",
                ));
                state.summary.support = crate::InterpreterSupport::Rejected(
                    crate::FrontendRejectionReason::EscapedEffect,
                );
            }
        }
        state
    }
}
