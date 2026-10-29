use etas_code::{
    CallableId,
    descriptor::{
        ParameterHandlerArmInfo, ParameterHandlerCompletionInfo, ParameterHandlerScopeInfo,
        ReturnObservationInfo, TypeInfoRef,
    },
};
use etas_effects::{
    ActionTraceDomain, CallableExitSummary, CallableOutcome, Effect, EffectSummary, EffectUnit,
    FinishSummary, ParameterCallTarget, ParameterExitFact, RequestInstantiationSummary,
    ResumeSummary, ReturnObservation,
};
use etas_hir::{HirExpr, HirExprId, HirOwner, HirPat, HirStmt, ResolveResult};
use etas_types::{PrimitiveType, Type, TypeId};

use super::{DefinitionDescriptorProjector, ProjectionError};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct CheckedHandlerArm {
    action: Effect,
    trace: ActionTraceDomain,
    result_type: TypeId,
    value: ReturnObservationInfo,
    finishes: bool,
}

pub(super) struct TraceProjectionContext<'a> {
    pub scope: usize,
    pub callable: &'a CallableId,
    pub signature: TypeInfoRef,
    pub parameters: &'a [etas_hir::SymbolId],
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
    pub(super) fn checked_handler_scope(
        &self,
        handler: HirExprId,
        exits: &CallableExitSummary,
        call_span: etas_core::Span,
    ) -> Option<Vec<CheckedHandlerArm>> {
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
        if handlers.len() != fact.arms.len() {
            return None;
        }
        let mut arms = Vec::new();
        for (arm, fact) in handlers.iter().zip(&fact.arms) {
            if fact.unit != (EffectUnit::HandlerArm { owner, arm: *arm }) {
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
            if checked_action.params.len() != data.patterns.len() {
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
            let (ty, observation, finishes) = match &exit.outcome {
                CallableOutcome::HandlerResumed { ty, observation }
                    if fact.resumable
                        && !signature.returns_never
                        && fact.completion.resumes == ResumeSummary::Once
                        && fact.completion.finishes == FinishSummary::None
                        && *ty == checked_action.output =>
                {
                    (ty, observation, false)
                }
                CallableOutcome::HandlerFinished { ty, observation }
                    if fact.completion.resumes == ResumeSummary::None
                        && fact.completion.finishes == FinishSummary::Once
                        && self.ignored_tail_handle(handler, call_span)
                        && self.handle_result_type(handler) == Some(*ty) =>
                {
                    (ty, observation, true)
                }
                _ => return None,
            };
            if exit.trace != summary.action_trace
                || !concrete_action_trace(&exit.trace)
                || fact.arm_effects != summary.escaping_effects
                || summary.requested_actions.open.is_some()
                || summary.requested_actions.effects != summary.action_trace.action_set()
                || !matches!(&summary.handler_requests, RequestInstantiationSummary::Available { parameters, .. } if parameters.is_empty())
            {
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
            expected.action_trace = summary.action_trace.clone();
            expected.requested_actions = summary.requested_actions.clone();
            expected.handler_requests = summary.handler_requests.clone();
            expected.default_actions = summary.default_actions.clone();
            expected.determinism = summary.determinism;
            expected.independent_determinism = summary.independent_determinism;
            expected.support = summary.support.clone();
            if summary != &expected {
                return None;
            }
            arms.push(CheckedHandlerArm {
                action: effect.clone(),
                trace: exit.trace.clone(),
                result_type: *ty,
                value,
                finishes,
            });
        }
        Some(arms)
    }

    pub(super) fn checked_parameter_handler_scopes(
        &self,
        exit: &ParameterExitFact,
        exits: &CallableExitSummary,
    ) -> Option<Vec<Vec<CheckedHandlerArm>>> {
        exit.enclosing_handlers
            .as_ref()?
            .iter()
            .map(|handler| self.checked_handler_scope(*handler, exits, exit.source.span()))
            .collect()
    }

    fn handle_result_type(&self, handler: HirExprId) -> Option<TypeId> {
        let HirOwner::Expr(handle) = *self.checked.hir.index().expr_owner.get(&handler)? else {
            return None;
        };
        self.checked.types.expr_types.get(&handle).copied()
    }

    fn ignored_tail_handle(&self, handler: HirExprId, call_span: etas_core::Span) -> bool {
        let Some(HirOwner::Expr(handle)) =
            self.checked.hir.index().expr_owner.get(&handler).copied()
        else {
            return false;
        };
        let Some(HirOwner::Stmt(stmt)) = self.checked.hir.index().expr_owner.get(&handle).copied()
        else {
            return false;
        };
        if !matches!(self.checked.stmts.get(stmt), Some(HirStmt::Expr { expr, .. }) if *expr == handle)
        {
            return false;
        }
        let Some(HirExpr::Handle {
            body,
            handler: actual,
            ..
        }) = self.checked.exprs.get(handle)
        else {
            return false;
        };
        if *actual != handler {
            return false;
        }
        let Some(HirExpr::Block(block)) = self.checked.exprs.get(*body) else {
            return false;
        };
        let Some(block) = self.checked.blocks.get(*block) else {
            return false;
        };
        let [stmt] = block.stmts.as_slice() else {
            return false;
        };
        if block.final_expr.is_some() {
            return false;
        }
        let Some(HirStmt::Expr { expr, .. }) = self.checked.stmts.get(*stmt) else {
            return false;
        };
        matches!(self.checked.exprs.get(*expr), Some(HirExpr::Call { span, .. }) if *span == call_span)
    }

    pub(super) fn project_parameter_handler_scopes(
        &mut self,
        target: ParameterCallTarget,
        span: etas_core::Span,
        exits: Option<&CallableExitSummary>,
        context: TraceProjectionContext<'_>,
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
                            action: self.analysis_effect(&arm.action, context.scope)?,
                            trace: self.analysis_trace(
                                &arm.trace,
                                context.callable,
                                context.signature,
                                context.parameters,
                                context.scope,
                                None,
                            )?,
                            completion: if arm.finishes {
                                ParameterHandlerCompletionInfo::Finish {
                                    result_type: self.type_ref(arm.result_type, context.scope)?,
                                    value: arm.value.clone(),
                                }
                            } else {
                                ParameterHandlerCompletionInfo::Resume {
                                    result_type: self.type_ref(arm.result_type, context.scope)?,
                                    value: arm.value.clone(),
                                }
                            },
                        })
                    })
                    .collect::<Result<Vec<_>, ProjectionError>>()?;
                Ok(ParameterHandlerScopeInfo::Handle { arms })
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }
}

fn concrete_action_trace(trace: &ActionTraceDomain) -> bool {
    let mut pending = vec![trace];
    while let Some(trace) = pending.pop() {
        match trace {
            ActionTraceDomain::Empty => {}
            ActionTraceDomain::Event(event) if event.action.is_action() => {}
            ActionTraceDomain::Seq(parts) | ActionTraceDomain::Choice(parts) => {
                pending.extend(parts)
            }
            ActionTraceDomain::Repeat(inner) => pending.push(inner),
            ActionTraceDomain::UnknownOrder(actions) if actions.iter().all(Effect::is_action) => {}
            _ => return false,
        }
    }
    true
}
