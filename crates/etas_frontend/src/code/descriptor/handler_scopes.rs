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

#[derive(Clone, PartialEq, Eq)]
pub(super) enum CheckedHandlerScope {
    Native(Vec<CheckedHandlerArm>),
    External(etas_effects::ParameterHandlerScopeRef),
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
    ) -> Option<Vec<CheckedHandlerScope>> {
        exit.enclosing_handlers
            .as_ref()?
            .iter()
            .map(|handler| match handler {
                etas_effects::ParameterHandlerScopeRef::Native(handler) => self
                    .checked_handler_scope(*handler, exits, exit.source.span())
                    .map(CheckedHandlerScope::Native),
                external @ etas_effects::ParameterHandlerScopeRef::External { .. } => {
                    self.checked.effects?.external_handler_scope(*external)?;
                    Some(CheckedHandlerScope::External(*external))
                }
            })
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
            self.checked_parameter_handler_scopes(exit, exits)
                .is_none_or(|other| !self.equivalent_handler_scopes(&scopes, &other))
        }) {
            return Ok(None);
        }
        scopes
            .iter()
            .map(|scope| {
                let arms = match scope {
                    CheckedHandlerScope::Native(arms) => arms,
                    CheckedHandlerScope::External(reference) => {
                        return self.project_external_handler_scope(*reference);
                    }
                };
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
                Ok(Some(ParameterHandlerScopeInfo::Handle { arms }))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|scopes| scopes.into_iter().collect())
    }

    fn equivalent_handler_scopes(
        &self,
        left: &[CheckedHandlerScope],
        right: &[CheckedHandlerScope],
    ) -> bool {
        if left.len() != right.len() {
            return false;
        }
        left.iter().zip(right).all(|(left, right)| {
            if left == right { return true; }
            let (CheckedHandlerScope::External(left), CheckedHandlerScope::External(right)) = (left, right) else { return false; };
            let (etas_effects::ParameterHandlerScopeRef::External { graph: left_id, .. },
                 etas_effects::ParameterHandlerScopeRef::External { graph: right_id, .. }) = (left, right) else { return false; };
            if left_id != right_id { return false; }
            let Some(facts) = self.checked.effects else { return false; };
            let (Some((graph, ParameterHandlerScopeInfo::Handle { arms: left })),
                 Some((_, ParameterHandlerScopeInfo::Handle { arms: right }))) =
                (facts.external_handler_scope(*left), facts.external_handler_scope(*right)) else { return false; };
            let Ok(graph) = etas_code::descriptor::DescriptorGraph::new(graph.clone(), self.limits) else { return false; };
            left.len() == right.len() && left.iter().zip(right).all(|(left, right)| {
                left.action == right.action
                    && std::mem::discriminant(&left.completion) == std::mem::discriminant(&right.completion)
                    && left.completion.value() == right.completion.value()
                    && matches!((graph.type_fingerprint(left.completion.result_type()), graph.type_fingerprint(right.completion.result_type())), (Ok(a), Ok(b)) if a == b)
                    && matches!((graph.trace_fingerprint(left.trace), graph.trace_fingerprint(right.trace)), (Ok(a), Ok(b)) if a == b)
            })
        })
    }

    fn project_external_handler_scope(
        &mut self,
        reference: etas_effects::ParameterHandlerScopeRef,
    ) -> Result<Option<ParameterHandlerScopeInfo>, ProjectionError> {
        use etas_code::descriptor::{
            DescriptorGraph, DescriptorGraphOffsets, DescriptorNode, StaticEffectArgInfo,
        };
        let Some((graph, scope)) = self
            .checked
            .effects
            .and_then(|facts| facts.external_handler_scope(reference))
        else {
            return Ok(None);
        };
        let graph = graph.clone();
        let ParameterHandlerScopeInfo::Handle { mut arms } = scope.clone();
        let mut roots = Vec::new();
        for arm in &arms {
            roots.push(DescriptorNode::Trace(arm.trace));
            roots.push(DescriptorNode::Type(arm.completion.result_type()));
            roots.extend(
                arm.action
                    .arguments
                    .iter()
                    .filter_map(|argument| match argument {
                        StaticEffectArgInfo::Type(ty) => Some(DescriptorNode::Type(*ty)),
                        _ => None,
                    }),
            );
        }
        let graph = DescriptorGraph::new(graph, self.limits)?;
        let offsets = DescriptorGraphOffsets {
            types: u32::try_from(self.types.len())
                .map_err(|_| ProjectionError::Invalid("type arena overflow".into()))?,
            rows: u32::try_from(self.rows.len())
                .map_err(|_| ProjectionError::Invalid("row arena overflow".into()))?,
            binders: u32::try_from(self.binders.len())
                .map_err(|_| ProjectionError::Invalid("binder arena overflow".into()))?,
            traces: u32::try_from(self.traces.len())
                .map_err(|_| ProjectionError::Invalid("trace arena overflow".into()))?,
        };
        let (segment, mapped) = graph.relocate_roots(&roots, offsets)?;
        if !segment.binders.is_empty() {
            return Ok(None);
        }
        self.ensure_nodes(
            segment
                .types
                .len()
                .saturating_add(segment.rows.len())
                .saturating_add(segment.traces.len()),
        )?;
        let mut mapped = mapped.into_iter();
        for arm in &mut arms {
            let Some(DescriptorNode::Trace(trace)) = mapped.next() else {
                return Err(ProjectionError::Invalid(
                    "missing relocated handler trace".into(),
                ));
            };
            let Some(DescriptorNode::Type(ty)) = mapped.next() else {
                return Err(ProjectionError::Invalid(
                    "missing relocated handler result".into(),
                ));
            };
            arm.trace = trace;
            match &mut arm.completion {
                ParameterHandlerCompletionInfo::Resume { result_type, .. }
                | ParameterHandlerCompletionInfo::Finish { result_type, .. } => *result_type = ty,
            }
            for argument in &mut arm.action.arguments {
                if let StaticEffectArgInfo::Type(ty) = argument {
                    let Some(DescriptorNode::Type(mapped_ty)) = mapped.next() else {
                        return Err(ProjectionError::Invalid(
                            "missing relocated handler selector".into(),
                        ));
                    };
                    *ty = mapped_ty;
                }
            }
        }
        self.types.extend(segment.types.into_iter().map(Some));
        self.rows.extend(segment.rows.into_iter().map(Some));
        self.traces.extend(segment.traces.into_iter().map(Some));
        Ok(Some(ParameterHandlerScopeInfo::Handle { arms }))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Frontend, ProjectInput, SourceInput,
        code::identity::{DefinitionIdentityIndex, DefinitionOrigins},
    };
    use etas_code::{
        CallableId, CodeFingerprint, DefinitionKey, DefinitionKind, PackageKey, descriptor::*,
    };
    use std::{collections::BTreeMap, sync::Arc};

    #[test]
    fn external_scope_projection_preserves_validated_action_trace() {
        let package = PackageKey::from_canonical_fingerprint(CodeFingerprint::from_bytes([7; 32]));
        let key = |name: &str, kind| {
            DefinitionKey::new(package, vec!["lib".into()], vec![name.into()], kind).unwrap()
        };
        let effect = key("Choice", DefinitionKind::Effect);
        let action = |name: &str| EffectInfo {
            target: EffectTargetInfo::Action {
                effect: effect.clone(),
                name: name.into(),
            },
            arguments: vec![],
        };
        let callable = CallableId::declaration(key("relay", DefinitionKind::Flow)).unwrap();
        let foreign = DescriptorGraphInput {
            types: vec![
                TypeInfo::Primitive(PrimitiveInfo::Unit),
                TypeInfo::Function(SignatureInfo {
                    generics: vec![],
                    parameters: vec![ParameterInfo {
                        name: Some("f".into()),
                        ty: TypeInfoRef(2),
                    }],
                    output: TypeInfoRef(0),
                    declared_output: None,
                    effects: None,
                }),
                TypeInfo::Function(SignatureInfo {
                    generics: vec![],
                    parameters: vec![],
                    output: TypeInfoRef(0),
                    declared_output: None,
                    effects: None,
                }),
            ],
            rows: vec![],
            binders: vec![],
            traces: vec![
                TraceInfo::Event(action("first")),
                TraceInfo::Event(action("second")),
                TraceInfo::Seq(vec![TraceInfoRef(0), TraceInfoRef(1)]),
                TraceInfo::ParameterCall(ParameterCallInfo {
                    callable,
                    signature: TypeInfoRef(1),
                    position: 0,
                    arguments: Some(vec![]),
                    handlers: Some(vec![ParameterHandlerScopeInfo::Handle {
                        arms: vec![ParameterHandlerArmInfo {
                            action: {
                                let mut request = action("request");
                                request
                                    .arguments
                                    .push(StaticEffectArgInfo::Type(TypeInfoRef(0)));
                                request
                            },
                            trace: TraceInfoRef(2),
                            completion: ParameterHandlerCompletionInfo::Resume {
                                result_type: TypeInfoRef(0),
                                value: ReturnObservationInfo::Unit,
                            },
                        }],
                    }]),
                }),
            ],
        };
        let original = DescriptorGraph::new(foreign.clone(), Default::default()).unwrap();
        let fingerprint = CodeFingerprint::from_bytes([9; 32]);
        let mut input = ProjectInput::single_source(SourceInput::anonymous(
            "module app; flow main() -> unit { return; }",
        ));
        input.options.entry_policy = crate::EntryPolicy::Optional;
        let output = Frontend.check_project(input);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let checked = output.checked.unwrap();
        let origins = DefinitionOrigins {
            source_modules: DefinitionOrigins::declared_source_modules(&checked, package).unwrap(),
            standard_package: package,
            external_definitions: BTreeMap::new(),
        };
        let identities = DefinitionIdentityIndex::build(&checked, &origins).unwrap();
        let mut facts = (*checked.effects).clone();
        facts
            .external_descriptor_graphs
            .insert(fingerprint, Arc::new(foreign));
        let input = super::super::DescriptorInput {
            effects: Some(&facts),
            ..(&*checked).into()
        };
        let mut projector =
            DefinitionDescriptorProjector::from_input(input, &identities, Default::default());
        projector
            .types
            .push(Some(TypeInfo::Primitive(PrimitiveInfo::Bool)));
        projector.traces.push(Some(TraceInfo::Empty));
        let reference = etas_effects::ParameterHandlerScopeRef::External {
            graph: fingerprint,
            parameter_call: 3,
            call_index: 0,
            scope: 0,
        };
        let ParameterHandlerScopeInfo::Handle { arms } = projector
            .project_external_handler_scope(reference)
            .unwrap()
            .unwrap();
        assert_eq!(arms.len(), 1);
        assert_eq!(
            arms[0].action.arguments,
            [StaticEffectArgInfo::Type(TypeInfoRef(1))]
        );
        assert_eq!(arms[0].completion.result_type(), TypeInfoRef(1));
        let projected_trace = arms[0].trace;
        let projected = projector.finish().unwrap();
        assert_eq!(
            original.trace_fingerprint(TraceInfoRef(2)).unwrap(),
            projected.trace_fingerprint(projected_trace).unwrap()
        );
        assert!(
            matches!(projected.trace(projected_trace), Some(TraceInfo::Seq(parts)) if parts.len() == 2)
        );
    }
}
