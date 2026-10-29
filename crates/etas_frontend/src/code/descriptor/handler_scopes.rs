use etas_code::descriptor::{
    ParameterHandlerArmInfo, ParameterHandlerScopeInfo, ReturnObservationInfo,
};
use etas_effects::{
    ActionTraceDomain, CallableExitSummary, CallableOutcome, Effect, EffectSummary, EffectUnit,
    FinishSummary, ParameterCallTarget, ParameterExitFact, ResumeSummary, ReturnObservation,
};
use etas_hir::{HirExpr, HirExprId, HirOwner, HirPat, ResolveResult};
use etas_types::{PrimitiveType, Type, TypeId};

use super::{DefinitionDescriptorProjector, ProjectionError};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct CheckedResumeArm {
    action: Effect,
    result_type: TypeId,
    value: ReturnObservationInfo,
}

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
    pub(super) fn checked_resume_scope(
        &self,
        handler: HirExprId,
        exits: &CallableExitSummary,
    ) -> Option<Vec<CheckedResumeArm>> {
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
        let mut arms = Vec::new();
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
            if signature.returns_never || checked_action.params.len() != data.patterns.len() {
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
            let CallableOutcome::HandlerResumed { ty, observation } = &exit.outcome else {
                return None;
            };
            if exit.trace != ActionTraceDomain::Empty || *ty != checked_action.output {
                return None;
            }
            let value = match (self.checked.type_store.get(*ty), observation) {
                (Some(Type::Primitive(PrimitiveType::Unit)), ReturnObservation::Unit) => {
                    ReturnObservationInfo::Unit
                }
                (Some(Type::Primitive(PrimitiveType::Bool)), ReturnObservation::Bool(value)) => {
                    ReturnObservationInfo::Bool(*value)
                }
                _ => return None,
            };
            let mut expected = EffectSummary::local();
            expected.exits = Some(completion.clone());
            if summary != &expected {
                return None;
            }
            arms.push(CheckedResumeArm {
                action: effect.clone(),
                result_type: *ty,
                value,
            });
        }
        Some(arms)
    }

    pub(super) fn checked_parameter_handler_scopes(
        &self,
        exit: &ParameterExitFact,
        exits: &CallableExitSummary,
    ) -> Option<Vec<Vec<CheckedResumeArm>>> {
        exit.enclosing_handlers
            .as_ref()?
            .iter()
            .map(|handler| self.checked_resume_scope(*handler, exits))
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
        let Some(scopes) = self.checked_parameter_handler_scopes(first, exits) else {
            return Ok(None);
        };
        if matching.any(|exit| {
            self.checked_parameter_handler_scopes(exit, exits).as_ref() != Some(&scopes)
        }) {
            return Ok(None);
        }
        scopes
            .iter()
            .map(|arms| {
                let arms = arms
                    .iter()
                    .map(|arm| {
                        Ok(ParameterHandlerArmInfo {
                            action: self.analysis_effect(&arm.action, scope)?,
                            result_type: self.type_ref(arm.result_type, scope)?,
                            value: arm.value.clone(),
                        })
                    })
                    .collect::<Result<Vec<_>, ProjectionError>>()?;
                Ok(ParameterHandlerScopeInfo::Resume { arms })
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }
}
