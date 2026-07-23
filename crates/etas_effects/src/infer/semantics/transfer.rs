use super::engine::EffectSemantics;
use super::shared::*;

impl HirAnalysisSemantics for EffectSemantics<'_> {
    type Domain = EffectState;

    fn hir(&self) -> &HirProgram {
        self.hir
    }

    fn incomplete_facts(&mut self, state: Self::Domain) -> Self::Domain {
        let anchor = state
            .unit
            .map(DiagnosticAnchor::Unit)
            .unwrap_or(DiagnosticAnchor::Project);
        self.incomplete_at_anchor(anchor, "effect analysis is missing checked facts", state)
    }

    fn after_expr(&mut self, expr: HirExprId, state: Self::Domain) -> Self::Domain {
        let mut state = state;
        if matches!(self.hir.exprs.get(expr), Some(HirExpr::Index { .. })) {
            let span = self.hir.exprs[expr].span(&self.hir.blocks);
            match self.types.facts.checked_index_errors.get(&expr).copied() {
                Some(error) => state.summary.record_escaping_effect(Effect::Error(error)),
                None if matches!(
                    self.types.facts.index_facts.get(&expr),
                    Some(etas_types::CheckedIndexKind::MapLookup { .. })
                ) => {}
                None => {
                    state = self.incomplete_at(
                        span,
                        "checked index expression requires a materialized IndexError fact",
                        state,
                    );
                }
            }
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
        self.inputs.expr_effects.insert(expr, state.summary.clone());
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
            HirStmt::Let { pat, value, .. } | HirStmt::Var { pat, value, .. } => {
                if let Some(sources) = self.latent_sources_for_expr(&state, value) {
                    self.record_latent_sources_for_pat(&mut state, pat, sources);
                } else if self.flow_container_missing_latent_sources(&state, value) {
                    let span = self.hir.exprs[value].span(&self.hir.blocks);
                    state = self.reject_effect_state(
                        span,
                        "local flow-value container requires checked latent source facts",
                        state,
                    );
                }
            }
            HirStmt::Resume { span, .. } => {
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
                }
            }
            HirStmt::Finish { span, .. } => {
                if state.handler_depth == 0 {
                    let _ = span;
                    state.summary.support =
                        InterpreterSupport::Rejected(FrontendRejectionReason::UnsupportedHandler);
                }
            }
            HirStmt::While { limits, span, .. } | HirStmt::Retry { limits, span, .. } => {
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
        state.record_anonymous_flow(owner, expr, EffectAnonymousFlowBody::Expr(expr));
        AnalysisStep::handled(state)
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
                .escaping_effects
                .remove_effect(&Effect::Error(escaping_error));
        } else {
            state = self.incomplete_at(
                self.hir.exprs[expr].span(&self.hir.blocks),
                "try expression requires checked try-capture facts",
                state,
            );
        }
        AnalysisStep::handled(state)
    }

    fn handle_expr(
        &mut self,
        _expr: HirExprId,
        _span: Span,
        state: Self::Domain,
    ) -> AnalysisStep<Self::Domain> {
        AnalysisStep::handled(state)
    }

    fn handle_expr_control(
        &mut self,
        expr: HirExprId,
        span: Span,
        _entry: Self::Domain,
        parts: HandleParts<Self::Domain>,
    ) -> etas_hir_analysis::intraprocedural::Control<Self::Domain> {
        self.handle_parts(expr, span, parts)
    }

    fn lambda_boundary(
        &mut self,
        expr: HirExprId,
        _span: Span,
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
