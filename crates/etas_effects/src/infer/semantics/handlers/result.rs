use std::collections::BTreeSet;

use etas_hir::HirItemId;

use super::*;
use crate::ActionTraceDomain;
use crate::facts::CallableValueOrigin;
use crate::infer::semantics::specialize::{
    EffectSpecializationError, TraceArgumentBinding, TraceArgumentBindings,
};

struct CheckedHandlerArm<'a> {
    fact: &'a HandlerArmFact,
    action: &'a crate::Effect,
    unconditional: bool,
}

impl EffectSemantics<'_> {
    /// Provider results belong to the invocation's handler scope. Keep callable
    /// parameters symbolic until that scope supplies their checked origins.
    pub(in crate::infer::semantics) fn bind_injected_callback_results(
        &self,
        application: HirExprId,
        handler: HirExprId,
        state: &mut EffectState,
    ) -> Result<(), EffectSpecializationError> {
        let calls = state.summary.action_trace.parameter_calls();
        if calls.is_empty() {
            return Ok(());
        }
        let Some((literal, _)) = self
            .immutable_handler_value(handler)
            .map_err(EffectSpecializationError::Interrupted)?
        else {
            return Ok(());
        };
        let Some(owner) = self.hir.index().expr_item.get(&literal).copied() else {
            return Ok(());
        };
        let Some(arms) = self.checked_handler_arms(literal, Some(owner))? else {
            return Ok(());
        };
        let coverage = EffectCoverage {
            registry: self.registry,
            types: &self.types.store,
        };
        let mut bindings = super::super::specialize::TraceArgumentBindings::new();
        for call in calls {
            let target = self
                .parameter_projection_target(call.target)
                .map(|(target, _)| target)
                .unwrap_or(call.target);
            let crate::ParameterCallTarget::Native(parameter) = target else {
                continue;
            };
            if !self.hir.parameter_injections.contains(&parameter) {
                continue;
            }
            let etas_hir::SymbolDef::Param { owner, .. } = self
                .hir
                .symbols
                .get(parameter)
                .expect("checked injected parameter")
                .def
            else {
                unreachable!()
            };
            let expr = self.hir.parameter_defaults[&parameter];
            if !contains_provider_request(
                &state.summary.action_trace,
                self.hir.exprs[expr].span(&self.hir.blocks),
            ) {
                continue;
            }
            let HirExpr::Perform { args, .. } = &self.hir.exprs[expr] else {
                unreachable!("injection is a provider request")
            };
            let Some(summary) = self.unit_summary(&EffectUnit::ParameterDefault {
                owner,
                parameter,
                expr,
            }) else {
                continue;
            };
            let Some(effect) = summary.requested_actions.effects.iter().next() else {
                continue;
            };
            let mut origins = BTreeSet::new();
            for arm in &arms {
                if !coverage.covers(arm.action, effect) {
                    continue;
                }
                let Some(resumed) = self
                    .unit_summary(&arm.fact.unit)
                    .and_then(|summary| summary.exits.as_ref())
                    .and_then(|exits| exits.handler_resume_sources())
                else {
                    continue;
                };
                let EffectUnit::HandlerArm { arm: id, .. } = arm.fact.unit else {
                    unreachable!()
                };
                let Some(payload) =
                    self.resume_payload_bindings(&self.hir.handler_arms[id].patterns, args)
                else {
                    continue;
                };
                let sources =
                    self.returned_callable_bindings(&resumed, &payload, state, application, None)?;
                origins.extend(sources);
                if arm.unconditional {
                    bindings.insert(target, TraceArgumentBinding::Origins(origins));
                    break;
                }
            }
        }
        if bindings.is_empty() {
            return Ok(());
        }
        let trace = state.summary.action_trace.clone();
        let mut summary = state.summary.clone();
        self.merge_parameter_call_summaries(
            &mut summary,
            &trace,
            &bindings,
            state,
            &BTreeSet::new(),
        )?;
        summary.action_trace = self.specialize_parameter_call_trace(&trace, &bindings, state)?;
        summary.exits = Some(state.exits.clone());
        self.solve_callback_completion(&mut summary, &bindings, &[], state)?;
        if let Some(exits) = &mut summary.exits {
            for exit in &mut exits.exits {
                exit.trace = self.specialize_parameter_call_trace(&exit.trace, &bindings, state)?;
            }
            for exit in &mut exits.parameter_exits {
                exit.prefix =
                    self.specialize_parameter_call_trace(&exit.prefix, &bindings, state)?;
            }
            state.exits = exits.clone();
        }
        state.summary = summary;
        Ok(())
    }

    fn checked_handler_arms(
        &self,
        handler: HirExprId,
        owner: Option<HirItemId>,
    ) -> Result<Option<Vec<CheckedHandlerArm<'_>>>, EffectSpecializationError> {
        self.read_expr_owner(handler);
        let (Some(HirExpr::Handler { handlers, .. }), Some(fact), Some(owner)) = (
            self.hir.exprs.get(handler),
            self.inputs.handler_values.get(&handler),
            owner,
        ) else {
            return Ok(None);
        };
        if !handler_arm_units_match(owner, handlers, fact.arms.iter().map(|fact| fact.unit)) {
            return Err(EffectSpecializationError::InvalidCallableEnvironment {
                reason: "handler arm evidence does not match its checked analysis unit".into(),
            });
        }
        let mut result = Vec::with_capacity(handlers.len());
        for (arm, fact) in handlers.iter().zip(&fact.arms) {
            if let Some(control) = self.query_control {
                control
                    .checkpoint()
                    .map_err(EffectSpecializationError::Interrupted)?;
            }
            let arm_data = self.hir.handler_arms.get(*arm).ok_or_else(|| {
                EffectSpecializationError::InvalidCallableEnvironment {
                    reason: "checked handler arm is missing from HIR".into(),
                }
            })?;
            let action = self.inputs.handler_action_effects.get(arm).ok_or_else(|| {
                EffectSpecializationError::InvalidCallableEnvironment {
                    reason: "checked handler arm is missing its action effect".into(),
                }
            })?;
            let mut unconditional = true;
            for pattern in &arm_data.patterns {
                if let Some(control) = self.query_control {
                    control
                        .checkpoint()
                        .map_err(EffectSpecializationError::Interrupted)?;
                }
                let pattern = self.hir.pats.get(*pattern).ok_or_else(|| {
                    EffectSpecializationError::InvalidCallableEnvironment {
                        reason: "checked handler arm is missing its payload pattern".into(),
                    }
                })?;
                unconditional &= matches!(
                    pattern,
                    etas_hir::HirPat::Binding { .. } | etas_hir::HirPat::Wildcard { .. }
                );
            }
            result.push(CheckedHandlerArm {
                fact,
                action,
                unconditional,
            });
        }
        Ok(Some(result))
    }

    pub(in crate::infer::semantics) fn record_resumed_request_sources(
        &self,
        request: HirExprId,
        effect: &crate::Effect,
        args: &[etas_hir::HirArg],
        state: &mut EffectState,
    ) -> Result<(), EffectSpecializationError> {
        if !self.expr_type_contains_flow(request) && !self.expr_type_has_parameters(request) {
            return Ok(());
        }
        let Some(owner) = state.owner else {
            return Ok(());
        };
        if self.resume_selectors_need_bindings(effect, owner)? {
            return Ok(());
        }
        let (_, Some(handlers)) = super::super::argument_values::enclosing_handlers(
            self.hir,
            request,
            self.query_control,
        )
        .map_err(EffectSpecializationError::Interrupted)?
        else {
            return Ok(());
        };
        let coverage = EffectCoverage {
            registry: self.registry,
            types: &self.types.store,
        };
        let mut sources = BTreeSet::new();
        for handler in handlers {
            let span = self.hir.exprs[handler].span(&self.hir.blocks);
            let Some(rows) = self.checked_handler_rows(handler, span) else {
                return Ok(());
            };
            if rows.handled.open.is_none()
                && !rows
                    .handled
                    .effects
                    .iter()
                    .any(|handled| coverage.covers(handled, effect))
            {
                continue;
            }
            let Some((literal, _)) = self
                .immutable_handler_value(handler)
                .map_err(EffectSpecializationError::Interrupted)?
            else {
                return Ok(());
            };
            if self.handler_captures_mutable_binding(literal)? {
                return Ok(());
            }
            let Some(owner) = self.hir.index().expr_item.get(&literal).copied() else {
                return Ok(());
            };
            let Some(arms) = self.checked_handler_arms(literal, Some(owner))? else {
                return Ok(());
            };
            for arm in arms {
                if self.resume_selectors_need_bindings(arm.action, owner)? {
                    // Substitution could make an earlier arm match this request.
                    return Ok(());
                }
                if !coverage.covers(arm.action, effect) {
                    continue;
                }
                let Some(exits) = self
                    .unit_summary(&arm.fact.unit)
                    .and_then(|summary| summary.exits.as_ref())
                else {
                    return Ok(());
                };
                let Some(resumed) = exits.handler_resume_sources() else {
                    return Ok(());
                };
                if !resumed.is_empty() {
                    let EffectUnit::HandlerArm { arm, .. } = arm.fact.unit else {
                        unreachable!()
                    };
                    let patterns = &self.hir.handler_arms[arm].patterns;
                    let Some(bindings) = self.resume_payload_bindings(patterns, args) else {
                        return Ok(());
                    };
                    sources.extend(
                        self.returned_callable_bindings(&resumed, &bindings, state, request, None)?,
                    );
                }
                if arm.unconditional {
                    state
                        .local_latent_values
                        .record_value_sources(request, sources);
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    pub(in crate::infer::semantics) fn named_provider_sources(
        &self,
        application: HirExprId,
        name: &str,
        ty: etas_types::TypeId,
        suffix: &[etas_code::descriptor::ValueProjectionStep],
        state: &EffectState,
    ) -> Option<BTreeSet<CallableValueOrigin>> {
        let request = crate::Effect::AppliedAction(crate::ActionInstanceRef {
            action: self.registry.action_by_name("std.effects.Inject.get")?,
            args: vec![etas_types::EffectArgRef::Type(ty)],
        });
        let coverage = EffectCoverage {
            registry: self.registry,
            types: &self.types.store,
        };
        let (_, handlers) = super::super::argument_values::enclosing_handlers(
            self.hir,
            application,
            self.query_control,
        )
        .ok()?;
        for handler in handlers? {
            let rows =
                self.checked_handler_rows(handler, self.hir.exprs[handler].span(&self.hir.blocks))?;
            if rows.handled.open.is_none()
                && !rows
                    .handled
                    .effects
                    .iter()
                    .any(|effect| coverage.covers(effect, &request))
            {
                continue;
            }
            let (literal, _) = self.immutable_handler_value(handler).ok()??;
            if self.handler_captures_mutable_binding(literal).ok()? {
                return None;
            }
            let owner = self.hir.index().expr_item.get(&literal).copied();
            for arm in self.checked_handler_arms(literal, owner).ok()?? {
                if !coverage.covers(arm.action, &request) {
                    continue;
                }
                let EffectUnit::HandlerArm { arm: id, .. } = arm.fact.unit else {
                    return None;
                };
                let data = &self.hir.handler_arms[id];
                let [pat] = data.patterns.as_slice() else {
                    return None;
                };
                // HIR orders exact provider names before the type fallback.
                match &self.hir.pats[*pat] {
                    etas_hir::HirPat::Literal(etas_hir::HirLiteral::String { value, .. })
                        if value != name =>
                    {
                        continue;
                    }
                    etas_hir::HirPat::Literal(etas_hir::HirLiteral::String { .. })
                    | etas_hir::HirPat::Wildcard { .. } => {}
                    _ => return None,
                }
                if !data.provider {
                    return None;
                }
                let exits = self.unit_summary(&arm.fact.unit)?.exits.as_ref()?;
                let bindings = TraceArgumentBindings::new();
                if suffix.is_empty() {
                    let returned = exits.handler_resume_sources()?;
                    return self
                        .returned_callable_bindings(&returned, &bindings, state, application, None)
                        .ok()
                        .map(|v| v.into_iter().collect());
                }
                let selected = super::super::return_projection::returned_positions(
                    exits
                        .value_projections
                        .iter()
                        .map(|(path, sources)| (path.as_slice(), sources)),
                    suffix,
                );
                if selected.is_empty() {
                    return None;
                }
                return selected.into_iter().try_fold(
                    BTreeSet::new(),
                    |mut found, (sources, remaining)| {
                        let crate::ProjectedReturnSources::Known(sources) = sources else {
                            return None;
                        };
                        found.extend(self.projected_return_bindings(
                            sources,
                            &remaining,
                            &bindings,
                            state,
                            application,
                            None,
                        )?);
                        Some(found)
                    },
                );
            }
        }
        None
    }

    fn resume_selectors_need_bindings(
        &self,
        effect: &crate::Effect,
        owner: HirItemId,
    ) -> Result<bool, EffectSpecializationError> {
        let crate::Effect::AppliedAction(action) = effect else {
            return Ok(false);
        };
        let Some(parameters) = self.native_generic_interface(owner) else {
            return Ok(true);
        };
        for argument in &action.args {
            let etas_types::EffectArgRef::Type(ty) = argument else {
                continue;
            };
            for (_, parameter) in &parameters {
                if let Some(etas_types::Type::Named(named)) =
                    self.types.store.get(parameter.subject)
                    && let Some(symbol) = named.source_parameter
                    && etas_types::type_contains_source_parameter(&self.types.store, *ty, symbol)?
                {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn handler_captures_mutable_binding(
        &self,
        handler: HirExprId,
    ) -> Result<bool, EffectSpecializationError> {
        use etas_hir::{HirItem, HirReceiverMode, HirStmt, SymbolDef};
        let captures = self.lambda_referenced_symbols(handler).map_err(|reason| {
            EffectSpecializationError::InvalidCallableEnvironment {
                reason: reason.into(),
            }
        })?;
        Ok(captures.into_iter().any(|symbol| {
            match self.hir.symbols.get(symbol).map(|symbol| &symbol.def) {
                Some(SymbolDef::Local { binding, .. }) => {
                    matches!(self.hir.stmts.get(*binding), Some(HirStmt::Var { .. }))
                }
                Some(SymbolDef::Param { owner, .. }) => {
                    matches!(self.hir.items.get(*owner), Some(HirItem::Flow(flow))
                        if flow.receiver_mode == HirReceiverMode::Updating
                            && flow.params.first() == Some(&symbol))
                }
                _ => false,
            }
        }))
    }

    fn resume_payload_bindings(
        &self,
        patterns: &[etas_hir::HirPatId],
        args: &[etas_hir::HirArg],
    ) -> Option<TraceArgumentBindings> {
        if patterns.len() != args.len() {
            return None;
        }
        let mut bindings = TraceArgumentBindings::new();
        for (pattern, arg) in patterns.iter().zip(args) {
            // Request parameter positions must be known before substituting a
            // handler's payload parameters.
            let etas_hir::HirArg::Positional(expr) = arg else {
                return None;
            };
            for (symbol, path) in self.pattern_projection_bindings(*pattern).ok()? {
                let argument = if path.is_empty() {
                    TraceArgumentBinding::Expression(*expr)
                } else {
                    TraceArgumentBinding::ProjectedExpression { expr: *expr, path }
                };
                bindings.insert(crate::ParameterCallTarget::Native(symbol), argument);
            }
        }
        Some(bindings)
    }

    pub(super) fn record_handle_result_sources(
        &self,
        expr: HirExprId,
        handler: HirExprId,
        body_normal: bool,
        state: &mut EffectState,
    ) -> Result<(), EffectSpecializationError> {
        let Some(HirExpr::Handle { body, .. }) = self.hir.exprs.get(expr) else {
            return Ok(());
        };
        if !self.expr_type_contains_flow(expr) && !self.symbolic_contract_expr(state, *body) {
            return Ok(());
        }
        let mut sources = BTreeSet::new();
        let mut stopped = crate::EffectSet::new();
        let arms = self.checked_handler_arms(handler, state.owner)?;
        if let Some(arms) = &arms {
            for arm in arms {
                if arm.fact.completion.resumes == crate::ResumeSummary::None
                    && arm.fact.completion.finishes != crate::FinishSummary::None
                    && arm.unconditional
                {
                    stopped.insert(arm.action.clone());
                }
            }
        }
        let coverage = crate::EffectCoverage {
            registry: self.registry,
            types: &self.types.store,
        };
        let body_normal = body_normal
            && may_avoid(
                &state.summary.action_trace,
                &|action| stopped.iter().any(|stop| coverage.covers(stop, action)),
                self.query_control,
            )?;
        if body_normal
            && arms.as_ref().is_some_and(|arms| {
                arms.iter()
                    .all(|arm| arm.fact.completion.finishes == crate::FinishSummary::None)
            })
        {
            let positions = state
                .local_latent_values
                .value_projections
                .range((*body, Vec::new())..)
                .take_while(|((id, _), _)| id == body)
                .map(|((_, path), sources)| ((expr, path.clone()), sources.clone()))
                .collect::<Vec<_>>();
            state
                .local_latent_values
                .value_projections
                .extend(positions);
        }
        if body_normal {
            match self.latent_sources_for_expr(state, *body) {
                Some(found) => sources.extend(found),
                None => {
                    sources.insert(CallableValueOrigin::Unavailable { expr: *body });
                }
            }
        }
        self.read_expr_owner(*body);
        match arms {
            Some(arms) => {
                for arm in arms {
                    if arm.fact.completion.finishes == crate::FinishSummary::None {
                        continue;
                    }
                    if let Some(crate::RequestInstantiationSummary::Available {
                        actions,
                        parameters,
                    }) = self.inputs.handled_body_requests.get(body)
                        && parameters.is_empty()
                        && actions
                            .iter()
                            .all(|request| !coverage.covers(arm.action, &request.action))
                    {
                        continue;
                    }
                    let finished = self
                        .unit_summary(&arm.fact.unit)
                        .and_then(|summary| summary.exits.as_ref())
                        .and_then(crate::CallableExitSummary::handler_finish_sources);
                    match finished {
                        Some(finished) => sources.extend(self.returned_callable_bindings(
                            &finished,
                            &Default::default(),
                            state,
                            expr,
                            None,
                        )?),
                        None => {
                            sources.insert(CallableValueOrigin::Unavailable { expr: handler });
                        }
                    }
                }
            }
            _ => {
                sources.insert(CallableValueOrigin::Unavailable { expr: handler });
            }
        }
        if sources.is_empty() {
            sources.insert(CallableValueOrigin::Unavailable { expr });
        }
        state
            .local_latent_values
            .record_value_sources(expr, sources);
        Ok(())
    }
}

fn handler_arm_units_match(
    owner: HirItemId,
    handlers: &[etas_hir::HirHandlerArmId],
    units: impl IntoIterator<Item = EffectUnit>,
) -> bool {
    handlers
        .iter()
        .map(|arm| EffectUnit::HandlerArm { owner, arm: *arm })
        .eq(units)
}

fn may_avoid(
    trace: &ActionTraceDomain,
    blocks: &impl Fn(&crate::Effect) -> bool,
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<bool, EffectSpecializationError> {
    enum Work<'a> {
        Trace(&'a ActionTraceDomain),
        Join { count: usize, sequence: bool },
    }
    let mut pending = vec![Work::Trace(trace)];
    let mut values = Vec::new();
    while let Some(work) = pending.pop() {
        if let Some(control) = control {
            control
                .checkpoint()
                .map_err(EffectSpecializationError::Interrupted)?;
        }
        match work {
            Work::Trace(ActionTraceDomain::Seq(parts) | ActionTraceDomain::Choice(parts))
                if !parts.is_empty() =>
            {
                pending.push(Work::Join {
                    count: parts.len(),
                    sequence: matches!(work, Work::Trace(ActionTraceDomain::Seq(_))),
                });
                pending.extend(parts.iter().rev().map(Work::Trace));
            }
            Work::Trace(ActionTraceDomain::Event(event)) => values.push(!blocks(&event.action)),
            // Widened/may-action footprints, symbolic calls and zero repetitions
            // do not prove that any particular handler is entered.
            Work::Trace(_) => values.push(true),
            Work::Join { count, sequence } => {
                let start = values.len().checked_sub(count).ok_or_else(|| {
                    EffectSpecializationError::InvalidCallableEnvironment {
                        reason: "action trace completion fold has missing children".into(),
                    }
                })?;
                let result = if sequence {
                    values[start..].iter().all(|value| *value)
                } else {
                    values[start..].iter().any(|value| *value)
                };
                values.truncate(start);
                values.push(result);
            }
        }
    }
    if values.len() != 1 {
        return Err(EffectSpecializationError::InvalidCallableEnvironment {
            reason: "action trace completion fold has no unique root".into(),
        });
    }
    Ok(values[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_ordered_must_events_prove_that_a_finishing_handler_is_entered() {
        let action = crate::Effect::Error(etas_types::TypeId(1));
        let event = ActionTraceDomain::Event(crate::ActionEvent {
            action: action.clone(),
            span: etas_core::Span::empty(etas_core::SourceId(1), etas_core::TextSize(0)),
            source: crate::ActionEventSource::Perform,
        });
        let blocks = |effect: &crate::Effect| *effect == action;
        assert!(!may_avoid(&event, &blocks, None).unwrap());
        assert!(
            may_avoid(
                &ActionTraceDomain::Choice(vec![event.clone(), ActionTraceDomain::Empty]),
                &blocks,
                None
            )
            .unwrap()
        );
        assert!(
            !may_avoid(
                &ActionTraceDomain::Seq(vec![
                    ActionTraceDomain::Choice(vec![event.clone(), ActionTraceDomain::Empty]),
                    event.clone()
                ]),
                &blocks,
                None
            )
            .unwrap()
        );
        assert!(may_avoid(&ActionTraceDomain::Repeat(Box::new(event)), &blocks, None).unwrap());
        assert!(
            may_avoid(
                &ActionTraceDomain::UnknownOrder(crate::EffectSet::one(action.clone())),
                &blocks,
                None
            )
            .unwrap()
        );
        assert!(
            may_avoid(
                &ActionTraceDomain::Widened {
                    actions: crate::EffectSet::one(action.clone()),
                    parameter_calls: vec![]
                },
                &blocks,
                None
            )
            .unwrap()
        );
        assert!(may_avoid(&ActionTraceDomain::Choice(vec![]), &blocks, None).unwrap());
    }

    #[test]
    fn handler_arm_evidence_requires_exact_owner_and_order() {
        let owner = HirItemId(1);
        let arms = [etas_hir::HirHandlerArmId(2), etas_hir::HirHandlerArmId(3)];
        let units = arms.map(|arm| EffectUnit::HandlerArm { owner, arm });
        assert!(handler_arm_units_match(owner, &arms, units));
        assert!(!handler_arm_units_match(owner, &arms, [units[1], units[0]]));
        assert!(!handler_arm_units_match(owner, &arms, [units[0]]));
        assert!(!handler_arm_units_match(HirItemId(4), &arms, units));
    }

    #[test]
    fn handler_arm_evidence_rejects_missing_action_and_wrong_unit() {
        let parsed = etas_syntax::parse_program(etas_core::SourceFile::anonymous(
            "effect Gate { action ask() -> bool; } flow run() -> bool { return handle { perform Gate.ask() } with { Gate.ask() => { resume true; } }; }",
        ));
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = etas_hir::HirRevision::try_new(etas_hir::lower_program(&parsed.value)).unwrap();
        let types = etas_types::check_program(&hir);
        assert!(types.diagnostics.is_empty(), "{:?}", types.diagnostics);
        let std = etas_std::standard_registry();
        let registry = crate::EffectRegistry::from_hir_types_dependencies_and_std(
            &hir,
            &types,
            &Default::default(),
            &std,
        )
        .unwrap();
        let output = crate::RunEffectPipeline::run(crate::EffectPipelineInput {
            external_defaults: &[],
            external_implementations: &[],
            source_snapshots: &[],
            residual_identity: None,
            trace_monitor_projector: None,
            frozen_calls: None,
            hir: &hir,
            types: &types,
            std_registry: &std,
            dependency_metadata: None,
            tool_bindings: &[],
            external_summaries: &[],
            external_trace_specs: &[],
            external_artifact_anchors: &[],
            reachable_items: None,
        })
        .unwrap()
        .effects;
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let mut semantics = EffectSemantics::new(&hir, &types, &std, &registry, &[], &[]);
        semantics.inputs.handler_values = output.facts.handler_values;
        semantics.inputs.handler_action_effects = output.facts.handler_action_effects;
        let handler = *semantics.inputs.handler_values.keys().next().unwrap();
        let fact = &semantics.inputs.handler_values[&handler];
        let EffectUnit::HandlerArm { owner, arm } = fact.arms[0].unit else {
            panic!("handler arm unit")
        };
        assert_eq!(
            semantics
                .checked_handler_arms(handler, Some(owner))
                .unwrap()
                .unwrap()
                .len(),
            1
        );
        let action = semantics
            .inputs
            .handler_action_effects
            .remove(&arm)
            .unwrap();
        assert!(matches!(
            semantics.checked_handler_arms(handler, Some(owner)),
            Err(EffectSpecializationError::InvalidCallableEnvironment { .. })
        ));
        semantics.inputs.handler_action_effects.insert(arm, action);
        semantics
            .inputs
            .handler_values
            .get_mut(&handler)
            .unwrap()
            .arms[0]
            .unit = EffectUnit::Item(owner);
        assert!(matches!(
            semantics.checked_handler_arms(handler, Some(owner)),
            Err(EffectSpecializationError::InvalidCallableEnvironment { .. })
        ));
    }

    #[test]
    fn trace_completion_analysis_propagates_cancellation() {
        struct Stop;
        impl etas_utils::PipelineExecutionControl for Stop {
            fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
                Err(etas_utils::PipelineInterruption {
                    kind: etas_utils::PipelineInterruptionKind::Cancelled,
                    reason: "cancel trace completion analysis".into(),
                })
            }
        }
        assert!(matches!(
            may_avoid(&ActionTraceDomain::Empty, &|_| false, Some(&Stop)),
            Err(EffectSpecializationError::Interrupted(interruption))
                if interruption.kind == etas_utils::PipelineInterruptionKind::Cancelled
        ));
    }
}

fn contains_provider_request(trace: &ActionTraceDomain, span: etas_core::Span) -> bool {
    let mut pending = vec![trace];
    let mut shared = std::collections::HashSet::new();
    while let Some(trace) = pending.pop() {
        match trace {
            ActionTraceDomain::Event(event)
                if event.span == span && event.source == crate::ActionEventSource::Perform =>
            {
                return true;
            }
            ActionTraceDomain::Seq(parts)
            | ActionTraceDomain::Choice(parts)
            | ActionTraceDomain::Parallel(parts) => pending.extend(parts),
            ActionTraceDomain::Repeat(body)
            | ActionTraceDomain::DefaultHandler { body, .. }
            | ActionTraceDomain::TaskStart { body, .. }
            | ActionTraceDomain::Recursive {
                body: Some(body), ..
            } => pending.push(body),
            ActionTraceDomain::Shared(node) if shared.insert(std::sync::Arc::as_ptr(node)) => {
                pending.push(&node.body)
            }
            _ => (),
        }
    }
    false
}
