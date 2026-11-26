use super::engine::EffectSemantics;
use super::shared::*;
use crate::infer::recursive_trace::TraceFact;
use etas_hir_analysis::intraprocedural::Control;

impl HirAnalysisSemantics for EffectSemantics<'_> {
    type Domain = EffectState;

    fn analysis_context(&self) -> &etas_hir_analysis::HirAnalysisContext {
        &self.context
    }

    fn incomplete_facts(&mut self, state: Self::Domain) -> Self::Domain {
        let anchor = state
            .unit
            .map(DiagnosticAnchor::Unit)
            .unwrap_or(DiagnosticAnchor::Project);
        self.incomplete_at_anchor(anchor, "effect analysis is missing checked facts", state)
    }

    fn begin_loop(&mut self, body: etas_hir::HirBlockId, mut state: Self::Domain) -> Self::Domain {
        state.summary.action_trace = self.loop_traces.begin(
            (state.unit, body, state.application),
            &state.summary.action_trace,
        );
        state
    }

    fn end_loop_control(
        &mut self,
        _body: etas_hir::HirBlockId,
        mut control: Control<Self::Domain>,
    ) -> Control<Self::Domain> {
        let equation = self.loop_traces.end();
        control.for_each_state_mut(|state| {
            equation.finish_summary(&mut state.summary);
            equation.finish_exits(&mut state.exits);
        });
        equation.finish_facts(&mut self.inputs);
        self.callable_summaries.get_mut().invalidate();
        control
    }

    fn join_loop_state(
        &mut self,
        _body: etas_hir::HirBlockId,
        state: &mut Self::Domain,
        back_edge: &Self::Domain,
    ) -> bool {
        let previous = state.clone();
        let header = if back_edge.unit.is_some() {
            Some(self.loop_traces.join(&back_edge.summary.action_trace))
        } else {
            None
        };
        etas_utils::JoinSemiLattice::join_assign(state, back_edge);
        if let Some(header) = header {
            state.summary.action_trace = header;
        }
        self.close_loop_origins(
            &previous.local_latent_values,
            &mut state.local_latent_values,
        );
        self.close_loop_origins(
            &previous.local_latent_containers,
            &mut state.local_latent_containers,
        );
        previous != *state
    }

    fn loop_fixpoint_incomplete(
        &mut self,
        body: etas_hir::HirBlockId,
        state: Self::Domain,
    ) -> Self::Domain {
        match self.hir.blocks.get(body) {
            Some(block) => {
                self.incomplete_at(block.span, "effect loop analysis did not converge", state)
            }
            None => self.incomplete_facts(state),
        }
    }

    fn bounded_for_iterations(&self, stmt: HirStmtId) -> Option<usize> {
        let HirStmt::For { iter, .. } = self.hir.stmts.get(stmt)? else {
            return None;
        };
        let elems = match self.hir.exprs.get(*iter)? {
            HirExpr::Array { elems, .. } | HirExpr::List { elems, .. } => elems,
            _ => return None,
        };
        let ty = *self.types.facts.expr_types.get(iter)?;
        (self.proven_non_callable_iteration_type(ty)
            && elems.len() <= etas_utils::IterationLimit::default().max_iterations())
        .then_some(elems.len().max(1))
    }

    fn after_bounded_for(&mut self, stmt: HirStmtId, mut state: Self::Domain) -> Self::Domain {
        if !state.exits.exits.is_empty()
            || state.summary.action_trace.has_unordered_approximation()
            || state
                .exits
                .exits
                .iter()
                .any(|exit| exit.trace.has_unordered_approximation())
            || state
                .exits
                .parameter_exits
                .iter()
                .any(|exit| exit.prefix.has_unordered_approximation())
        {
            state
                .exits
                .gaps
                .insert(crate::OutcomeAnalysisGap::Loop(stmt));
        }
        state
    }

    fn bounded_for_break(&mut self, stmt: HirStmtId, mut state: Self::Domain) -> Self::Domain {
        state
            .exits
            .gaps
            .insert(crate::OutcomeAnalysisGap::Loop(stmt));
        state
    }

    fn before_expr(&mut self, expr: HirExprId, mut state: Self::Domain) -> Self::Domain {
        self.record_agent_construction(expr, &mut state);
        self.inputs.source_call_arguments.remove(&expr);
        self.inputs.dynamic_call_sources.remove(&expr);
        if let Some(unit) = state.unit {
            self.call_completion_exits.remove(&(unit, expr));
        }
        state
    }

    fn begin_limit_bounds(&mut self, _expr: HirExprId, mut state: Self::Domain) -> Self::Domain {
        state.summary = EffectSummary::default();
        state
    }

    fn finish_limit_bounds(
        &mut self,
        expr: HirExprId,
        entry: Self::Domain,
        mut control: Control<Self::Domain>,
    ) -> Control<Self::Domain> {
        let span = self.hir.exprs[expr].span(&self.hir.blocks);
        let mut invalid = false;
        control.for_each_state_mut(|state| {
            if !state.summary.requested_actions.is_empty()
                || !state.summary.escaping_effects.is_empty()
                || state.summary.determinism != crate::Determinism::Deterministic
                || state.summary.support != InterpreterSupport::LocalOnly
                || state.incomplete
            {
                invalid = true;
                state.summary.support =
                    InterpreterSupport::Rejected(FrontendRejectionReason::MissingRequirement);
            }
            let mut combined = entry.summary.clone();
            combined.seq_assign(&state.summary);
            state.summary = combined;
        });
        if invalid {
            self.diagnostics.push(Diagnostic::effect_check(EffectDiagnosticCode::MissingOrInvalidLimit, span,
                "local @limits arguments must be deterministic, action-free, and proven not to suspend"));
        }
        control
    }

    fn begin_match_arm(
        &mut self,
        expr: HirExprId,
        scrutinee: HirExprId,
        pattern: HirPatId,
        mut state: Self::Domain,
    ) -> Self::Domain {
        match self.latent_sources_for_expr(&state, scrutinee) {
            Some(sources) => {
                if let Err(reason) = self.record_latent_sources_for_pat(
                    &mut state,
                    pattern,
                    Some(sources),
                    Some(scrutinee),
                ) {
                    let span = self.hir.exprs[expr].span(&self.hir.blocks);
                    return self.incomplete_at(span, reason, state);
                }
            }
            None if self.expr_type_contains_flow(scrutinee) => {
                let span = self.hir.exprs[expr].span(&self.hir.blocks);
                return self.incomplete_at(
                    span,
                    "match payload requires checked latent callable origins",
                    state,
                );
            }
            None => {}
        }
        state
    }

    fn after_expr(&mut self, expr: HirExprId, mut state: Self::Domain) -> Self::Domain {
        self.record_autonomous_start(expr, &mut state);
        self.update_expr_state(expr, &mut state);
        state
    }

    fn after_expr_control(
        &mut self,
        expr: HirExprId,
        mut control: etas_hir_analysis::intraprocedural::Control<Self::Domain>,
    ) -> etas_hir_analysis::intraprocedural::Control<Self::Domain> {
        if let Some(state) = control.normal_state_mut() {
            self.record_autonomous_start(expr, state);
        }
        control.for_each_state_mut(|state| self.update_expr_state(expr, state));
        let completion = control
            .normal_state()
            .and_then(|state| state.unit)
            .and_then(|unit| self.call_completion_exits.remove(&(unit, expr)));
        if let Some(exits) = completion {
            if self.pure_exprs.remove(&expr) {
                self.inputs.expr_effects.entry(expr).or_default();
            }
            if let Some(fact) = self.inputs.expr_effects.get_mut(&expr) {
                fact.exits = Some(exits.clone());
                self.record_trace_fact(TraceFact::Expr(expr));
            }
            if exits.has_only_terminal_exits() {
                control.with_normal_processed(|mut state| {
                    state.exits.join_assign(&exits);
                    etas_hir_analysis::intraprocedural::Control::error(state)
                })
            } else {
                control
            }
        } else if self.expr_is_never(expr) {
            control.with_normal_processed(|mut state| {
                self.record_terminal_expr(expr, &mut state);
                etas_hir_analysis::intraprocedural::Control::error(state)
            })
        } else {
            control
        }
    }

    fn return_value(
        &mut self,
        _: HirStmtId,
        value: Option<HirExprId>,
        mut state: Self::Domain,
    ) -> Self::Domain {
        self.record_return(value, &mut state);
        state
    }

    fn begin_for_body(
        &mut self,
        _: HirStmtId,
        iterable: HirExprId,
        pattern: HirPatId,
        mut state: Self::Domain,
    ) -> Self::Domain {
        use etas_code::descriptor::ValueProjectionStep as Step;
        let span = self.hir.exprs[iterable].span(&self.hir.blocks);
        let cursor = self.types.facts.expr_types.get(&iterable).and_then(|ty| {
            etas_types::source_iteration::checked_cursor_indexed(
                &self.types.store,
                &self.types.facts,
                self.type_lookup(),
                *ty,
            )
            .ok()
            .flatten()
        });
        if cursor.is_some() {
            let next = self.projected_sources(iterable, &[Step::Field("next".into())], &state);
            if let Some(next) = &next {
                self.inputs
                    .iteration_call_sources
                    .insert(iterable, next.clone());
            }
            let next = next.unwrap_or_default().into_iter().collect::<Vec<_>>();
            if let Err(reason) =
                self.record_iteration_pattern_sources(pattern, &next, iterable, &mut state)
            {
                return self.incomplete_at(span, reason, state);
            }
            return state;
        }
        let sources = self
            .projected_sources(iterable, &[Step::SequenceElement], &state)
            .or_else(|| {
                self.latent_sources_for_expr(&state, iterable)
                    .map(|sources| sources.into_iter().collect())
            });
        match sources {
            Some(sources) => {
                if let Err(reason) = self.record_latent_sources_for_pat(
                    &mut state,
                    pattern,
                    Some(sources.into_iter().collect()),
                    None,
                ) {
                    return self.incomplete_at(span, reason, state);
                }
            }
            None if self.expr_type_contains_flow(iterable) => {
                return self.incomplete_at(
                    span,
                    "iteration element is missing checked value origins",
                    state,
                );
            }
            None => {}
        }
        state
    }

    fn begin_handler_arm(&mut self, arm: HirHandlerArmId, mut state: Self::Domain) -> Self::Domain {
        state.handler_depth += 1;
        self.bind_inference_decoder(arm, &mut state);
        state
    }

    fn after_stmt(&mut self, stmt: HirStmtId, mut state: Self::Domain) -> Self::Domain {
        let Some(stmt_data) = self.hir.stmts.get(stmt).cloned() else {
            return state;
        };
        match stmt_data {
            HirStmt::Assign {
                target,
                value,
                span,
            } if self.is_typed_flow_expr(target)
                || self.expr_type_contains_flow(target)
                || self.expr_type_has_parameters(target)
                || self
                    .types
                    .facts
                    .expr_types
                    .get(&target)
                    .is_some_and(|ty| self.stable_agent_state_layout(*ty).is_some())
                || self.symbolic_contract_expr(&state, value) =>
            {
                if let Err(message) = self.record_callable_assignment(&mut state, target, value) {
                    state = self.incomplete_at(span, message, state);
                }
            }
            HirStmt::For { .. } => {
                state
                    .exits
                    .gaps
                    .insert(crate::OutcomeAnalysisGap::Loop(stmt));
            }
            HirStmt::Let { pat, value, .. } | HirStmt::Var { pat, value, .. } => {
                if self.flow_container_missing_latent_sources(&state, value) {
                    let span = self.hir.exprs[value].span(&self.hir.blocks);
                    state = self.reject_effect_state(
                        span,
                        "local flow-value container requires checked latent source facts",
                        state,
                    );
                }
                let sources = self.latent_sources_for_expr(&state, value);
                if let Err(reason) =
                    self.record_latent_sources_for_pat(&mut state, pat, sources, Some(value))
                {
                    let span = self.hir.exprs[value].span(&self.hir.blocks);
                    state = self.incomplete_at(span, reason, state);
                }
            }
            HirStmt::Resume { value, span } => {
                if state.handler_depth == 0 {
                    self.diagnostics.push(Diagnostic::effect_check(
                        EffectDiagnosticCode::ResumeOutsideHandler,
                        span,
                        "resume is only valid inside a handler arm",
                    ));
                    state.summary.support =
                        InterpreterSupport::Rejected(FrontendRejectionReason::UnsupportedHandler);
                } else {
                    state.resume_count += 1;
                    self.record_resume_value(value, &mut state);
                }
            }
            HirStmt::Finish { value, span } => {
                if state.handler_depth == 0 {
                    let _ = span;
                    state.summary.support =
                        InterpreterSupport::Rejected(FrontendRejectionReason::UnsupportedHandler);
                } else {
                    self.record_finish_value(value, &mut state);
                }
            }
            HirStmt::While { .. } | HirStmt::Retry { .. } => {
                state
                    .exits
                    .gaps
                    .insert(crate::OutcomeAnalysisGap::Loop(stmt));
            }
            _ => {}
        }
        if state.summary == EffectSummary::default()
            && !self.inputs.stmt_effects.contains_key(&stmt)
        {
            self.pure_stmts.insert(stmt);
            return state;
        }
        self.record_trace_fact(TraceFact::Stmt(stmt));
        self.inputs.stmt_effects.insert(stmt, state.summary.clone());
        state
    }

    fn perform(
        &mut self,
        expr: HirExprId,
        action: &ResolvedActionRef,
        generic_args: &[HirGenericArg],
        args: &[HirArg],
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(self.perform_action(expr, action, generic_args, args, span, state))
    }

    fn bind_call_arguments(&mut self, call: HirExprId, state: Self::Domain) -> Self::Domain {
        self.apply_omitted_argument_effects(call, state)
    }

    fn direct_call(
        &mut self,
        _call: HirExprId,
        _callee: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::unhandled(state, "direct_call")
    }

    fn method_call(
        &mut self,
        expr: HirExprId,
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        match self
            .types
            .facts
            .checked_method_calls
            .get(&expr)
            .map(|fact| &fact.target)
        {
            Some(
                etas_types::method::MethodTarget::BoundSpec { .. }
                | etas_types::method::MethodTarget::External { .. },
            ) => self.spec_method_call(expr, span, state),
            Some(etas_types::method::MethodTarget::Standard { .. }) => {
                AnalysisStep::handled(self.std_method_call(expr, span, state))
            }
            _ => AnalysisStep::handled(self.incomplete_at(
                span,
                "method call has no scheduled checked behavior target",
                state,
            )),
        }
    }

    fn spec_method_call(
        &mut self,
        expr: HirExprId,
        span: Span,
        mut state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        self.record_agent_dispatch(expr, &mut state);
        match self.spec_method_call_evidence(expr, &state) {
            Ok(super::spec_methods::MethodEvidence::Resolved(
                source @ LatentSource::Metadata(_),
            )) => match self
                .latent_source_summary(source, span)
                .and_then(|summary| self.specialize_source_memory_method(expr, summary))
                .and_then(|summary| {
                    if self.is_bound_agent_request(expr) {
                        self.agent_request_summary(source, span, summary)
                    } else {
                        Ok(summary)
                    }
                }) {
                Ok(summary) => {
                    if let LatentSource::Metadata(source) = source {
                        if let Err(error) =
                            self.record_applied_metadata_method_returns(source, expr, &mut state)
                        {
                            return AnalysisStep::handled(self.incomplete_at(
                                span,
                                error.to_string(),
                                state,
                            ));
                        }
                        self.inputs
                            .metadata_call_targets
                            .entry(expr)
                            .or_default()
                            .insert(source);
                    }
                    self.note_call_outcomes(expr, None, &summary, &mut state);
                    state.summary.seq_assign(&summary);
                    AnalysisStep::handled(state)
                }
                Err(error) => {
                    AnalysisStep::handled(self.incomplete_at(span, error.to_string(), state))
                }
            },
            Ok(super::spec_methods::MethodEvidence::Resolved(source)) => {
                self.apply_method_sources(expr, span, state, std::iter::once(source))
            }
            Ok(super::spec_methods::MethodEvidence::Alternatives(sources)) => {
                self.apply_method_sources(expr, span, state, sources)
            }
            Ok(super::spec_methods::MethodEvidence::Contract { summary }) => {
                self.inputs.contract_method_calls.insert(expr, ());
                self.note_call_outcomes(expr, None, &summary, &mut state);
                state.summary.seq_assign(&summary);
                AnalysisStep::handled(state)
            }
            Ok(super::spec_methods::MethodEvidence::Deferred { summary }) => {
                let ActionTraceDomain::ParameterCall { target, .. } = &summary.action_trace else {
                    return AnalysisStep::handled(self.incomplete_at(
                        span,
                        "deferred method completion has no checked invocation target",
                        state,
                    ));
                };
                let Some(output) = self.types.facts.expr_types.get(&expr).copied() else {
                    return AnalysisStep::handled(self.incomplete_at(
                        span,
                        "deferred method completion has no checked output type",
                        state,
                    ));
                };
                state
                    .exits
                    .gaps
                    .insert(crate::OutcomeAnalysisGap::ParameterCall {
                        target: *target,
                        output,
                        source: span.into(),
                    });
                state.summary.seq_assign(&summary);
                AnalysisStep::handled(state)
            }
            Err(error) => AnalysisStep::handled(self.incomplete_at(span, error.to_string(), state)),
        }
    }

    fn stage_compose(
        &mut self,
        expr: HirExprId,
        span: Span,
        mut state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        if matches!(
            state.unit,
            Some(EffectUnit::AnonymousFlow { value, .. }) if value == expr
        ) {
            return AnalysisStep::handled(self.apply_stage_effects(expr, span, state));
        }
        let Some(owner) = state.owner else {
            state.mark_incomplete();
            return AnalysisStep::handled(state);
        };
        let unit = EffectUnit::AnonymousFlow {
            owner,
            value: expr,
            body: EffectAnonymousFlowBody::Expr(expr),
        };
        let mut captured = super::state::LatentEnv::default();
        if let Some(HirExpr::StageCompose { stages, .. }) = self.hir.exprs.get(expr) {
            for stage in stages {
                if let Some(sources) = self.latent_sources_for_expr(&state, stage.expr) {
                    captured.record_value_sources(stage.expr, sources);
                }
            }
        }
        self.discovered_value_captures
            .entry(unit)
            .or_default()
            .join_assign(&captured);
        state.record_anonymous_flow(owner, expr, EffectAnonymousFlowBody::Expr(expr));
        AnalysisStep::handled(state)
    }

    fn stage_targets_are_captured(&self, expr: HirExprId, state: &Self::Domain) -> bool {
        matches!(state.unit, Some(EffectUnit::AnonymousFlow { value, .. }) if value == expr)
    }

    fn pipeline(
        &mut self,
        expr: HirExprId,
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(self.apply_stage_effects(expr, span, state))
    }

    fn try_expr(
        &mut self,
        expr: HirExprId,
        _span: Span,
        mut state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        if !self.discarded_try_completion(expr) {
            state
                .exits
                .gaps
                .insert(crate::OutcomeAnalysisGap::Try(expr));
        }
        if let Some(fact) = self.types.facts.try_facts.get(&expr) {
            let escaping_errors = state
                .summary
                .escaping_effects
                .effects
                .iter()
                .filter_map(|effect| match effect {
                    Effect::Error(error) => Some(*error),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let Some((captured_error, escaping_error)) =
                self.try_capture_error(fact.target_error, &escaping_errors)
            else {
                let message = if escaping_errors.len() > 1 {
                    "try expression cannot capture from an operand with multiple Error[E] effects; use an explicit handler to select or translate the error"
                } else {
                    "try expression requires a checked captured Error[E] effect"
                };
                self.diagnostics.push(Diagnostic::effect_check(
                    EffectDiagnosticCode::InvalidTryCapture,
                    self.hir.exprs[expr].span(&self.hir.blocks),
                    message,
                ));
                state.mark_incomplete();
                return AnalysisStep::handled(state);
            };
            self.inputs.try_captures.insert(
                expr,
                crate::TryCaptureFact {
                    operand: fact.operand,
                    captured_error,
                    result_type: fact.result_type,
                    conversions: Vec::new(),
                },
            );
            state
                .summary
                .remove_escaping_effect(&Effect::Error(escaping_error));
        } else {
            state = self.incomplete_at(
                self.hir.exprs[expr].span(&self.hir.blocks),
                "try expression requires checked try-capture facts",
                state,
            );
        }
        AnalysisStep::handled(state)
    }

    fn begin_try_operand(&mut self, _expr: HirExprId, mut state: Self::Domain) -> Self::Domain {
        state.summary = EffectSummary::local();
        state
    }

    fn try_expr_control(
        &mut self,
        expr: HirExprId,
        span: Span,
        entry: Self::Domain,
        mut control: etas_hir_analysis::intraprocedural::Control<Self::Domain>,
    ) -> etas_hir_analysis::intraprocedural::Control<Self::Domain> {
        let error = control.take_error_state();
        let mut control = control.with_normal_processed(|state| {
            let state = match self.try_expr(expr, span, state) {
                AnalysisStep::Handled(state) => state,
                AnalysisStep::Unhandled { state, .. } => self.incomplete_facts(state),
            };
            etas_hir_analysis::intraprocedural::Control::normal(state)
        });
        if let Some(state) = error {
            let escaping = state
                .summary
                .escaping_effects
                .effects
                .iter()
                .filter_map(|effect| match effect {
                    Effect::Error(error) => Some(*error),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let target = self
                .types
                .facts
                .try_facts
                .get(&expr)
                .and_then(|fact| fact.target_error);
            if self.try_capture_error(target, &escaping).is_some() {
                let state = match self.try_expr(expr, span, state) {
                    AnalysisStep::Handled(state) => state,
                    AnalysisStep::Unhandled { state, .. } => self.incomplete_facts(state),
                };
                control.join_normal(state);
            } else {
                control.join_error(state);
            }
        }
        control.map_states(|mut state| {
            let mut summary = entry.summary.clone();
            summary.seq_assign(&state.summary);
            state.summary = summary;
            state
        })
    }

    fn handle_expr(
        &mut self,
        _expr: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(state)
    }

    fn begin_handle_body(&mut self, _expr: HirExprId, mut state: Self::Domain) -> Self::Domain {
        state.summary = EffectSummary::local();
        state.exits = crate::CallableExitSummary::default();
        state
    }

    fn handle_expr_control(
        &mut self,
        expr: HirExprId,
        span: Span,
        entry: Self::Domain,
        parts: HandleParts<Self::Domain>,
    ) -> etas_hir_analysis::intraprocedural::Control<Self::Domain> {
        self.handle_parts(expr, span, entry, parts)
    }

    fn lambda_boundary(
        &mut self,
        expr: HirExprId,
        span: Span,
        mut state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        let Some(owner) = state.owner else {
            state.mark_incomplete();
            return AnalysisStep::handled(state);
        };
        let body = match self.hir.exprs.get(expr) {
            Some(HirExpr::Lambda { body, .. }) => match body {
                etas_hir::HirLambdaBody::Expr(expr) => EffectAnonymousFlowBody::Expr(*expr),
                etas_hir::HirLambdaBody::Block(block) => EffectAnonymousFlowBody::Block(*block),
            },
            _ => {
                state.mark_incomplete();
                return AnalysisStep::handled(state);
            }
        };
        let captures = match self.lambda_value_captures(expr, &state) {
            Ok(captures) => captures,
            Err(reason) => return AnalysisStep::handled(self.incomplete_at(span, reason, state)),
        };
        self.discovered_value_captures
            .entry(EffectUnit::AnonymousFlow {
                owner,
                value: expr,
                body,
            })
            .or_default()
            .join_assign(&captures);
        state.record_anonymous_flow(owner, expr, body);
        AnalysisStep::handled(state)
    }

    fn handler_boundary(
        &mut self,
        expr: HirExprId,
        span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        let Some(HirExpr::Handler { handlers, .. }) = self.hir.exprs.get(expr) else {
            return AnalysisStep::handled(self.incomplete_at(
                span,
                "handler boundary requires a handler expression",
                state,
            ));
        };
        AnalysisStep::handled(self.materialize_handler_value(expr, span, handlers, state))
    }
}

impl EffectSemantics<'_> {
    fn update_expr_state(&mut self, expr: HirExprId, state: &mut EffectState) {
        self.record_structural_latent_result(expr, state);
        self.record_confined_agent_state(expr, state);
        if let Err(error) = self.record_checked_read_sources(expr, state) {
            self.mark_incomplete_at(
                self.hir.exprs[expr].span(&self.hir.blocks),
                error.to_string(),
                state,
            );
        }
        self.validate_callback_arguments(expr, state);
        self.note_operation_outcomes(expr, state);
        if matches!(
            self.hir.exprs.get(expr),
            Some(HirExpr::Limited {
                target: etas_hir::HirLimitTarget::While(_) | etas_hir::HirLimitTarget::Retry(_),
                ..
            })
        ) {
            if let Some(error) = self.types.facts.known_std_types.budget_exceeded {
                state.summary.record_escaping_effect(Effect::Error(error));
            } else {
                self.mark_incomplete_at(
                    self.hir.exprs[expr].span(&self.hir.blocks),
                    "local loop budget requires the checked std BudgetExceeded type",
                    state,
                );
            }
        }
        if let Some(HirExpr::Infer { action, .. }) = self.hir.exprs.get(expr) {
            let span = action.span;
            let Some(fact) = self.types.facts.agent_inferences.get(&expr) else {
                self.mark_incomplete_at(
                    span,
                    "agent inference requires checked owner, prompt and output facts",
                    state,
                );
                return;
            };
            let Some((invocation, summary)) = self.agent_infer_summary(expr, fact, action, span)
            else {
                self.mark_incomplete_at(
                    span,
                    "agent inference requires its canonical action descriptor and solved model-tool summaries",
                    state,
                );
                return;
            };
            self.inputs.performed_actions.insert(
                expr,
                crate::PerformedActionFact {
                    expr,
                    invocation,
                    effect_segments: action
                        .effect
                        .path
                        .segments
                        .iter()
                        .map(|segment| segment.name.clone())
                        .collect(),
                    action: action.action.clone(),
                    action_symbol: Some(fact.action_symbol),
                    args: vec![fact.prompt],
                    span,
                    summary: summary.clone(),
                },
            );
            state.summary.seq_assign(&summary);
        }
        if let Some(error) = self.types.facts.checked_index_errors.get(&expr).copied() {
            state.summary.record_escaping_effect(Effect::Error(error));
        } else if matches!(
            self.hir.exprs.get(expr),
            Some(HirExpr::Index { .. } | HirExpr::Slice { .. })
        ) && !matches!(
            self.types.facts.index_facts.get(&expr),
            Some(etas_types::CheckedIndexKind::MapLookup { .. })
        ) {
            let span = self.hir.exprs[expr].span(&self.hir.blocks);
            self.mark_incomplete_at(
                span,
                "checked index expression requires a materialized IndexError fact",
                state,
            );
        }
        if let Some(EffectUnit::Item(item)) = state.unit {
            if matches!(
                self.hir.items.get(item),
                Some(etas_hir::HirItem::TopLevelLet(value)) if value.value == Some(expr)
            ) && self.flow_container_missing_latent_sources(state, expr)
            {
                let span = self.hir.exprs[expr].span(&self.hir.blocks);
                self.mark_rejected_at(
                    span,
                    "top-level flow-value container requires checked latent source facts",
                    state,
                );
            }
        }
        // Keep an already published slot: a trace equation may be watching it.
        if state.summary == EffectSummary::default()
            && !self.inputs.expr_effects.contains_key(&expr)
        {
            self.pure_exprs.insert(expr);
            return;
        }
        let exits = state.summary.exits.take();
        let fact = state.summary.clone();
        state.summary.exits = exits;
        self.record_trace_fact(TraceFact::Expr(expr));
        self.inputs.expr_effects.insert(expr, fact);
    }

    fn proven_non_callable_iteration_type(&self, root: TypeId) -> bool {
        let mut pending = vec![root];
        let mut visited = std::collections::HashSet::new();
        while let Some(ty) = pending.pop() {
            if !visited.insert(ty) {
                continue;
            }
            if self.descriptor_contract_type(ty).is_some() {
                return false;
            }
            match self.types.store.get(ty) {
                Some(Type::Primitive(_) | Type::IntegerLiteral { .. }) => {}
                Some(
                    Type::Array(inner)
                    | Type::List(inner)
                    | Type::Set(inner)
                    | Type::Slice(inner)
                    | Type::Option(inner)
                    | Type::Schema(inner)
                    | Type::Message(inner)
                    | Type::MemorySelection(inner)
                    | Type::MemoryRegion(inner)
                    | Type::Refined { base: inner, .. }
                    | Type::Trust { inner, .. }
                    | Type::Range { index: inner },
                ) => pending.push(*inner),
                Some(
                    Type::Map { key, value }
                    | Type::Store { key, value }
                    | Type::Result {
                        ok: key,
                        err: value,
                    },
                ) => pending.extend([*key, *value]),
                Some(Type::Record(record)) => {
                    pending.extend(record.fields.iter().map(|field| field.ty))
                }
                Some(Type::Tuple(fields)) => pending.extend(fields),
                Some(Type::Nominal(nominal)) => match nominal.representation {
                    Some(representation) => pending.push(representation),
                    None => return false,
                },
                Some(Type::Enum(_)) => match self.types.facts.enum_layouts.get(&ty) {
                    Some(layout) => pending.extend(
                        layout
                            .variants
                            .iter()
                            .flat_map(|variant| variant.fields.iter())
                            .copied(),
                    ),
                    None => return false,
                },
                Some(
                    Type::Existential { .. }
                    | Type::Var(_)
                    | Type::Function(_)
                    | Type::Handler(_)
                    | Type::Task(_)
                    | Type::SchedulerHandle
                    | Type::Continuation(_)
                    | Type::Named(_)
                    | Type::Applied { .. }
                    | Type::Prompt
                    | Type::PromptPart
                    | Type::MemoryPlace(_)
                    | Type::ResourceHandle(_),
                )
                | Some(Type::AgentContext(_))
                | None => return false,
            }
        }
        true
    }
}

impl EffectSemantics<'_> {
    fn apply_method_sources(
        &mut self,
        expr: HirExprId,
        span: Span,
        mut state: EffectState,
        sources: impl IntoIterator<Item = LatentSource>,
    ) -> AnalysisStep<EffectState> {
        let mut joined: Option<crate::EffectSummary> = None;
        let managed = self.is_bound_agent_request(expr);
        for source in sources {
            let summary = match self
                .latent_source_summary(source, span)
                .and_then(|summary| {
                    if managed {
                        self.agent_request_summary(source, span, summary)
                    } else {
                        Ok(summary)
                    }
                }) {
                Ok(summary) => summary,
                Err(error) => {
                    return AnalysisStep::handled(self.incomplete_at(
                        span,
                        error.to_string(),
                        state,
                    ));
                }
            };
            match source {
                LatentSource::Captured { unit, .. } | LatentSource::Unit(unit) => {
                    self.inputs
                        .native_call_targets
                        .entry(expr)
                        .or_default()
                        .insert(unit);
                }
                LatentSource::Metadata(source) => {
                    self.inputs
                        .metadata_call_targets
                        .entry(expr)
                        .or_default()
                        .insert(source);
                }
                _ => {}
            }
            match &mut joined {
                Some(joined) => {
                    joined.join_branch(&summary);
                }
                None => joined = Some(summary),
            }
        }
        let Some(summary) = joined else {
            return AnalysisStep::handled(self.incomplete_at(
                span,
                "method has no checked implementation sources",
                state,
            ));
        };
        self.note_call_outcomes(expr, None, &summary, &mut state);
        state.summary.seq_assign(&summary);
        AnalysisStep::handled(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonconverged_loop_reports_its_real_body_span() {
        let source = etas_core::SourceFile::anonymous("flow f() -> unit { return; }");
        let parsed = etas_syntax::parse_program(source);
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let hir = etas_hir::HirRevision::try_new(etas_hir::lower_program(&parsed.value)).unwrap();
        let (body, block) = hir.blocks.iter().next().unwrap();
        let span = block.span;
        let types = etas_types::TypeOutput::default();
        let std = etas_std::standard_registry();
        let registry = crate::EffectRegistry::with_standard_effects_from(&std);
        let mut semantics = EffectSemantics::new(&hir, &types, &std, &registry, &[], &[]);

        let state = semantics.loop_fixpoint_incomplete(body, EffectState::default());
        assert!(state.incomplete);
        let [diagnostic] = semantics.diagnostics.as_slice() else {
            panic!("expected exactly one diagnostic");
        };
        assert_eq!(diagnostic.primary.span, span);
        assert!(diagnostic.message.contains("did not converge"));
    }
}
