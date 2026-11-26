use super::engine::EffectSemantics;
use crate::{EffectSummary, EffectUnit, infer::analysis::dependencies::InputFact};
use etas_hir::{HirExprId, HirItemId};
use std::collections::{HashMap, HashSet};

impl EffectSemantics<'_> {
    pub(crate) fn select_reader(&mut self, owner: Option<HirItemId>) {
        self.dependencies.select(
            owner,
            (
                self.diagnostics.len(),
                self.diagnostic_materialization_errors.len(),
                self.requested_types.get_mut().len(),
            ),
        );
    }

    pub(super) fn read_unit(&self, unit: EffectUnit) {
        if let Some(owner) = unit.owner() {
            self.dependencies.read(InputFact::Owner(owner));
        }
    }

    pub(super) fn unit_summary(&self, unit: &EffectUnit) -> Option<&EffectSummary> {
        self.read_unit(*unit);
        self.inputs.unit_effects.get(unit)
    }

    pub(super) fn read_expr_owner(&self, expr: HirExprId) {
        if let Some(owner) = self.hir.index().expr_item.get(&expr) {
            self.dependencies.read(InputFact::BodyFacts(*owner));
        }
    }

    pub(super) fn read_arm_owner(&self, arm: etas_hir::HirHandlerArmId) {
        if let Some(etas_hir::HirOwner::Expr(expr)) = self.hir.index().handler_arm_owner.get(&arm) {
            self.read_expr_owner(*expr);
        }
    }

    /// Reuse the facts published by clean bodies. Start dirty bodies with the
    /// same empty local outputs as a fresh solve, including optional facts
    /// which may disappear once a callback target is known.
    pub(crate) fn reuse_clean(
        &mut self,
        mut old: Self,
        dirty: &HashSet<HirItemId>,
    ) -> HashMap<EffectUnit, EffectSummary> {
        let index = self.hir.index();
        let clean_expr =
            |expr: &HirExprId| !index.expr_item.get(expr).is_some_and(|i| dirty.contains(i));
        let clean_stmt = |stmt: &etas_hir::HirStmtId| {
            !index.stmt_item.get(stmt).is_some_and(|i| dirty.contains(i))
        };
        let clean_unit = |unit: &EffectUnit| !unit.owner().is_some_and(|i| dirty.contains(&i));

        macro_rules! retain_exprs {
            ($($field:ident),+ $(,)?) => {$(
                old.inputs.$field.retain(|key, _| clean_expr(key));
                if self.inputs.$field.is_empty() {
                    self.inputs.$field = old.inputs.$field;
                } else {
                    self.inputs.$field.extend(old.inputs.$field);
                }
            )+};
        }
        retain_exprs!(
            handled_body_requests,
            native_call_targets,
            contract_method_calls,
            source_call_arguments,
            dynamic_call_sources,
            iteration_call_sources,
            metadata_call_targets,
            expr_effects,
            handler_values,
            handle_applications,
            performed_actions,
            try_captures,
            latent_effects,
            std_callable_values,
            std_action_calls
        );
        self.inputs.stmt_effects = old.inputs.stmt_effects;
        self.inputs.stmt_effects.retain(|key, _| clean_stmt(key));
        let (clean_summaries, invalidated_summaries) = old
            .inputs
            .unit_effects
            .into_iter()
            .partition(|(unit, _)| clean_unit(unit));
        self.inputs.unit_effects = clean_summaries;
        self.inputs.handler_action_effects = old.inputs.handler_action_effects;
        self.inputs.handler_action_effects.retain(|arm, _| matches!(
            index.handler_arm_owner.get(arm), Some(etas_hir::HirOwner::Expr(expr)) if clean_expr(expr)
        ));
        self.inputs.deferred_first_class_calls = old.inputs.deferred_first_class_calls;
        self.inputs
            .deferred_first_class_calls
            .retain(|unit, _| clean_unit(unit));
        self.inputs.unresolved_parameter_calls = old.inputs.unresolved_parameter_calls;
        self.inputs.unresolved_parameter_calls.retain(|_, calls| {
            calls.retain(clean_expr);
            !calls.is_empty()
        });
        self.inputs.latent_realizations = old.inputs.latent_realizations;
        self.inputs.latent_realizations.retain(|_, calls| {
            calls.retain(clean_expr);
            !calls.is_empty()
        });
        self.pure_exprs = old.pure_exprs;
        self.pure_exprs.retain(clean_expr);
        self.pure_stmts = old.pure_stmts;
        self.pure_stmts.retain(clean_stmt);
        self.generic_effect_rows = old.generic_effect_rows;
        self.generic_effect_rows
            .get_mut()
            .retain(|key, _| clean_expr(key));
        self.frozen_call_targets = old.frozen_call_targets;
        self.frozen_call_targets.retain(|key, _| clean_expr(key));
        self.discovered_spec_targets = old.discovered_spec_targets;
        self.discovered_spec_targets
            .get_mut()
            .retain(|key, _| clean_expr(key));
        self.discovered_call_results = old.discovered_call_results;
        self.discovered_call_results
            .get_mut()
            .retain_values(clean_expr);
        self.trace_equations
            .reuse_closed_equations(old.trace_equations, dirty);
        self.dependencies = old.dependencies;
        self.dependencies.retain(dirty);
        invalidated_summaries
    }
}
