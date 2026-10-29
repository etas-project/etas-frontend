use super::engine::EffectSemantics;
use super::shared::*;

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

    fn before_expr(&mut self, expr: HirExprId, state: Self::Domain) -> Self::Domain {
        if let Some(unit) = state.unit {
            self.call_completion_exits.remove(&(unit, expr));
        }
        state
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
                if let Err(reason) =
                    self.record_latent_sources_for_pat(&mut state, pattern, sources)
                {
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

    fn after_expr(&mut self, expr: HirExprId, state: Self::Domain) -> Self::Domain {
        let mut state = state;
        self.record_structural_latent_result(expr, &mut state);
        self.note_operation_outcomes(expr, &mut state);
        if let Some(HirExpr::Infer { action, .. }) = self.hir.exprs.get(expr) {
            let span = action.span;
            let Some(fact) = self.types.facts.agent_inferences.get(&expr) else {
                return self.incomplete_at(
                    span,
                    "agent inference requires checked owner, prompt and output facts",
                    state,
                );
            };
            let Some((invocation, summary)) = self.agent_infer_summary(fact, action, span) else {
                return self.incomplete_at(
                    span,
                    "agent inference requires its canonical action descriptor and solved model-tool summaries",
                    state,
                );
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
        } else if matches!(self.hir.exprs.get(expr), Some(HirExpr::Index { .. }))
            && !matches!(
                self.types.facts.index_facts.get(&expr),
                Some(etas_types::CheckedIndexKind::MapLookup { .. })
            )
        {
            let span = self.hir.exprs[expr].span(&self.hir.blocks);
            state = self.incomplete_at(
                span,
                "checked index expression requires a materialized IndexError fact",
                state,
            );
        }
        if let Some(EffectUnit::Item(item)) = state.unit {
            if matches!(
                self.hir.items.get(item),
                Some(etas_hir::HirItem::TopLevelLet(value)) if value.value == expr
            ) && self.flow_container_missing_latent_sources(&state, expr)
            {
                let span = self.hir.exprs[expr].span(&self.hir.blocks);
                state = self.reject_effect_state(
                    span,
                    "top-level flow-value container requires checked latent source facts",
                    state,
                );
            }
        }
        let mut fact = state.summary.clone();
        fact.exits = None;
        self.inputs.expr_effects.insert(expr, fact);
        state
    }

    fn after_expr_control(
        &mut self,
        expr: HirExprId,
        control: etas_hir_analysis::intraprocedural::Control<Self::Domain>,
    ) -> etas_hir_analysis::intraprocedural::Control<Self::Domain> {
        let control = control.map_states(|state| self.after_expr(expr, state));
        let completion = control
            .normal_state()
            .and_then(|state| state.unit)
            .and_then(|unit| self.call_completion_exits.remove(&(unit, expr)));
        if let Some(exits) = completion {
            if let Some(fact) = self.inputs.expr_effects.get_mut(&expr) {
                fact.exits = Some(exits.clone());
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

    fn begin_handler_arm(
        &mut self,
        _arm: HirHandlerArmId,
        mut state: Self::Domain,
    ) -> Self::Domain {
        state.handler_depth += 1;
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
                if let Some(sources) = self.latent_sources_for_expr(&state, value) {
                    if let Err(reason) =
                        self.record_latent_sources_for_pat(&mut state, pat, sources)
                    {
                        let span = self.hir.exprs[value].span(&self.hir.blocks);
                        state = self.incomplete_at(span, reason, state);
                    }
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
            HirStmt::While { limits, span, .. } | HirStmt::Retry { limits, span, .. } => {
                state
                    .exits
                    .gaps
                    .insert(crate::OutcomeAnalysisGap::Loop(stmt));
                if limits.is_empty() {
                    self.diagnostics.push(Diagnostic::effect_check(
                        EffectDiagnosticCode::MissingEffectLoopLimit,
                        span,
                        "effectful loops and retry blocks require an explicit limit",
                    ));
                    state.summary.support =
                        InterpreterSupport::Rejected(FrontendRejectionReason::MissingRequirement);
                    return state;
                }
                let mut parsed = RequirementSet::new();
                for limit in limits {
                    let Some(requirement) = self.limit_requirement_from_expr(limit) else {
                        self.diagnostics.push(Diagnostic::effect_check(
                            EffectDiagnosticCode::MissingOrInvalidLimit,
                            span,
                            "loop limit requires a checked standard limit constructor",
                        ));
                        state.summary.support = InterpreterSupport::Rejected(
                            FrontendRejectionReason::MissingRequirement,
                        );
                        return state;
                    };
                    parsed.insert(RequirementFact::Limit(requirement));
                }
                state.summary.requirements.union_assign(&parsed);
            }
            _ => {}
        }
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
        AnalysisStep::handled(self.std_method_call(expr, span, state))
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
