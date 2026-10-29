use etas_code::descriptor::ParameterHandlerScopeInfo;
use etas_effects::{
    ActionTraceDomain, CallableExitSummary, CallableOutcome, Effect, EffectRow, EffectSet,
    EffectSummary, EffectUnit, FinishSummary, ParameterCallTarget, ParameterExitFact,
    ResumeSummary, ReturnObservation,
};
use etas_hir::{HirExpr, HirExprId, HirOwner, HirPat, ResolveResult};
use etas_types::{PrimitiveType, Type};

use super::{DefinitionDescriptorProjector, ProjectionError};

impl DefinitionDescriptorProjector<'_> {
    pub(super) fn handler_scope_source(&self, handler: HirExprId) -> Option<etas_core::Span> {
        let HirOwner::Expr(handle) = *self.checked.hir.index().expr_owner.get(&handler)? else {
            return None;
        };
        let expression = self.checked.exprs.get(handle)?;
        let HirExpr::Handle {
            handler: actual, ..
        } = expression
        else {
            return None;
        };
        (*actual == handler).then(|| expression.span(&self.checked.blocks))
    }

    /// Resolve a scope only from checked handler and handler-arm facts. An
    /// unavailable transformer stays unavailable; it is never an empty scope.
    pub(super) fn checked_resume_unit_scope(
        &self,
        handler: HirExprId,
        exits: &CallableExitSummary,
    ) -> Option<EffectRow> {
        let span = self.handler_scope_source(handler)?;
        let mut evidence = exits
            .handler_gaps
            .iter()
            .filter(|gap| gap.source.span() == span)
            .peekable();
        evidence.peek()?;
        if evidence.any(|gap| !gap.normal_control) {
            return None;
        }
        let effects = self.checked.effects?;
        let HirExpr::Handler { handlers, .. } = self.checked.exprs.get(handler)? else {
            return None;
        };
        let fact = effects.handler_values.get(&handler)?;
        let owner = *self.checked.hir.index().expr_item.get(&handler)?;
        if handlers.len() != fact.arms.len() || !fact.produced.is_empty() {
            return None;
        }
        let mut actions = EffectSet::new();
        for (arm, fact) in handlers.iter().zip(&fact.arms) {
            if fact.unit != (EffectUnit::HandlerArm { owner, arm: *arm })
                || !fact.resumable
                || fact.completion.resumes != ResumeSummary::Once
                || fact.completion.finishes != FinishSummary::None
                || fact.completion.terminates_never
            {
                return None;
            }
            let data = self.checked.handler_arms.get(*arm)?;
            if !data.patterns.iter().all(|pattern| {
                matches!(
                    self.checked.pats.get(*pattern),
                    Some(HirPat::Binding { .. } | HirPat::Wildcard { .. })
                )
            }) {
                return None;
            }
            let effect = effects.handler_action_effects.get(arm)?;
            let (action, selectors) = match effect {
                Effect::Action(action) => (action, &[][..]),
                Effect::AppliedAction(instance) => (&instance.action, instance.args.as_slice()),
                _ => return None,
            };
            let ResolveResult::Resolved(symbol) = data.action.action_symbol else {
                return None;
            };
            if fact.action.action_symbol != Some(symbol) {
                return None;
            }
            let checked_action = self.checked.types.handler_actions.get(arm)?;
            if checked_action.action_symbol != symbol || checked_action.selectors != selectors {
                return None;
            }
            self.checked.effect_registry.action_signature(action)?;
            let signature = self.checked.types.action_signatures.get(&symbol)?;
            if signature.returns_never
                || checked_action.params.len() != data.patterns.len()
                || !matches!(
                    self.checked.type_store.get(checked_action.output),
                    Some(Type::Primitive(PrimitiveType::Unit))
                )
            {
                return None;
            }
            let summary = effects.unit_effects.get(&fact.unit)?;
            let completion = summary.exits.as_ref()?;
            if !completion.gaps.is_empty()
                || !completion.handler_gaps.is_empty()
                || !completion.parameter_exits.is_empty()
                || completion.exits.len() != 1
            {
                return None;
            }
            let exit = &completion.exits[0];
            if exit.trace != ActionTraceDomain::Empty
                || !matches!(exit.outcome, CallableOutcome::HandlerResumed {
                    ty,
                    observation: ReturnObservation::Unit,
                } if matches!(self.checked.type_store.get(ty), Some(Type::Primitive(PrimitiveType::Unit))))
            {
                return None;
            }
            let mut expected = EffectSummary::local();
            expected.exits = Some(completion.clone());
            if summary != &expected {
                return None;
            }
            actions.insert(effect.clone());
        }
        Some(EffectRow::closed(actions))
    }

    pub(super) fn checked_parameter_handler_rows(
        &self,
        exit: &ParameterExitFact,
        exits: &CallableExitSummary,
    ) -> Option<Vec<EffectRow>> {
        exit.enclosing_handlers
            .as_ref()?
            .iter()
            .map(|handler| self.checked_resume_unit_scope(*handler, exits))
            .collect()
    }

    pub(super) fn project_parameter_handler_scopes(
        &mut self,
        target: ParameterCallTarget,
        span: etas_core::Span,
        exits: Option<&CallableExitSummary>,
        scope: usize,
    ) -> Result<Option<Vec<ParameterHandlerScopeInfo>>, ProjectionError> {
        let Some(exits) = exits else {
            return Ok(None);
        };
        let mut matching = exits
            .parameter_exits
            .iter()
            .filter(|exit| exit.target == target && exit.source.span() == span);
        let Some(first) = matching.next() else {
            return Ok(None);
        };
        let Some(rows) = self.checked_parameter_handler_rows(first, exits) else {
            return Ok(None);
        };
        if matching
            .any(|exit| self.checked_parameter_handler_rows(exit, exits).as_ref() != Some(&rows))
        {
            return Ok(None);
        }
        rows.iter()
            .map(|row| {
                self.analysis_row(row, scope)
                    .map(|actions| ParameterHandlerScopeInfo::ResumeUnit { actions })
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }
}
