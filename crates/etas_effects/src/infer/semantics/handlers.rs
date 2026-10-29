use etas_core::{Diagnostic, EffectDiagnosticCode, Span};
use etas_hir::{HirBlockId, HirExpr, HirExprId, HirHandlerArmId, HirStmt};
use etas_hir_analysis::intraprocedural::{Control, HandleParts, HandlerParts};
use etas_types::{HandlerProducedEffects, PrimitiveType, Type, TypeId};

use crate::{
    EffectCoverage, EffectRow, EffectSet, EffectSummary, FrontendRejectionReason,
    HandleApplicationFact, HandlerActionFact, HandlerArmFact, HandlerCompletionSummary,
    HandlerValueFact, HandlerValueRef, HostRequirementKind, InterpreterSupport, ResumeSummary,
};

use crate::infer::unit::EffectUnit;

use super::engine::EffectSemantics;
use super::state::EffectState;

mod completion;
mod result;

fn handler_evaluation_is_inert(
    entry: &EffectState,
    handler: &EffectState,
    expected: &EffectSummary,
) -> bool {
    !handler.incomplete && handler.exits == entry.exits && &handler.summary == expected
}

impl EffectSemantics<'_> {
    fn handler_evaluation_is_inert(
        &self,
        expr: HirExprId,
        entry: &EffectState,
        handler: &EffectState,
    ) -> bool {
        if handler.incomplete || handler.exits != entry.exits {
            return false;
        }
        let mut expected = entry.summary.clone();
        if matches!(self.hir.exprs.get(expr), Some(HirExpr::Handler { .. })) {
            let Some(fact) = self.inputs.handler_values.get(&expr) else {
                return false;
            };
            for arm in &fact.arms {
                let Some(summary) = self.inputs.unit_effects.get(&arm.unit) else {
                    return false;
                };
                expected.merge_request_footprint_without_escaping_or_trace(summary);
            }
        }
        handler_evaluation_is_inert(entry, handler, &expected)
    }

    pub(crate) fn handle_parts(
        &mut self,
        expr: HirExprId,
        span: Span,
        entry: EffectState,
        parts: HandleParts<EffectState>,
    ) -> Control<EffectState> {
        match parts.handler {
            HandlerParts::Expr {
                expr: handler,
                control,
            } => {
                let only_normal = parts.body.normal_state().is_some()
                    && parts.body.return_state().is_none()
                    && parts.body.break_state().is_none()
                    && parts.body.continue_state().is_none()
                    && parts.body.resume_state().is_none()
                    && parts.body.finish_state().is_none()
                    && parts.body.error_state().is_none();
                let body_normal = parts.body.normal_state().is_some();
                let handler_only_normal = control.normal_state().is_some()
                    && control.return_state().is_none()
                    && control.break_state().is_none()
                    && control.continue_state().is_none()
                    && control.resume_state().is_none()
                    && control.finish_state().is_none()
                    && control.error_state().is_none();
                let mut control = control;
                let Some(mut handler_state) = control.take_normal_state() else {
                    return control;
                };
                let body_state = parts.body.into_joined_domain();
                if let Some(HirExpr::Handle { body, .. }) = self.hir.exprs.get(expr) {
                    self.inputs
                        .handled_body_requests
                        .insert(*body, body_state.summary.handler_requests.clone());
                }
                if let Some(summary) = self.handler_value_item_summary(handler) {
                    handler_state
                        .summary
                        .merge_request_footprint_without_escaping_or_trace(&summary);
                }
                let rows = match self.handler_rows_for_expr(handler, span) {
                    Ok(rows) => rows,
                    Err(interruption) => {
                        self.callable_summaries.borrow_mut().interruption = Some(interruption);
                        control.join_normal(handler_state);
                        return control;
                    }
                };
                let Some((handled, produced)) = rows else {
                    let mut result = body_state;
                    let mut summary = handler_state.summary;
                    summary.seq_assign(&result.summary);
                    result.summary = summary;
                    result.exits.join_assign(&handler_state.exits);
                    control.join_normal(self.incomplete_at(
                        span,
                        "handled expression requires checked handler rows",
                        result,
                    ));
                    return control;
                };
                let coverage = EffectCoverage {
                    registry: self.registry,
                    types: &self.types.store,
                };
                let definitely_handled = match self.definitely_handled_actions(handler) {
                    Ok(row) => row,
                    Err(interruption) => {
                        self.callable_summaries.borrow_mut().interruption = Some(interruption);
                        control.join_normal(handler_state);
                        return control;
                    }
                };
                let value_completion = only_normal
                    && !handler_state.incomplete
                    && self.checked_value_handler_completion(
                        expr,
                        handler,
                        &entry,
                        &body_state,
                        (&handled, &produced),
                    );
                let mut result = body_state;
                if !value_completion {
                    result.exits.handler_gaps.insert(crate::HandlerGapEvidence {
                        source: span.into(),
                        normal_control: only_normal
                            && handler_only_normal
                            && !result.incomplete
                            && self.handler_evaluation_is_inert(handler, &entry, &handler_state),
                        handled: handled.clone(),
                        body_escaping: result.summary.escaping_effects.clone(),
                    });
                }
                let mut remaining_requests = result.summary.handler_requests.clone();
                remaining_requests.subtract_handled(&coverage, &definitely_handled);
                result.summary.subtract_handled_effects(&coverage, &handled);
                result.summary.handler_requests = remaining_requests;
                let remaining = result.summary.escaping_effects.clone();
                if !value_completion {
                    result
                        .exits
                        .gaps
                        .insert(crate::OutcomeAnalysisGap::Handler(span.into()));
                }
                result.summary.default_actions =
                    coverage.subtract_handled(&result.summary.default_actions, &definitely_handled);
                result.summary.record_handled_action_row(handled.clone());
                self.remove_handled_runtime_support(&mut result.summary, &handled);
                self.apply_runtime_support_for_row(&mut result.summary, &remaining);
                let active_defaults = result.summary.default_actions.clone();
                self.apply_runtime_support_for_row(&mut result.summary, &active_defaults);
                result
                    .summary
                    .merge_request_footprint_without_escaping_or_trace(&handler_state.summary);
                result
                    .summary
                    .handler_requests
                    .union_assign(&entry.summary.handler_requests);
                result.summary.record_escaping_effect_row(produced.clone());
                self.apply_runtime_support_for_row(&mut result.summary, &produced);
                result
                    .summary
                    .require_runtime(crate::RuntimeRequirementReason::RuntimeHandler);
                self.inputs.handle_applications.insert(
                    expr,
                    HandleApplicationFact {
                        handler: HandlerValueRef::Expr { expr: handler },
                        handled,
                        produced,
                        remaining,
                    },
                );
                if let Err(error) =
                    self.record_handle_result_sources(expr, handler, body_normal, &mut result)
                {
                    if let super::specialize::EffectSpecializationError::Interrupted(interruption) =
                        error
                    {
                        self.callable_summaries.borrow_mut().interruption = Some(interruption);
                        control.join_normal(result);
                        return control;
                    }
                    result = self.incomplete_at(span, error.to_string(), result);
                }
                for exit in &mut result.exits.exits {
                    let mut trace = handler_state.summary.action_trace.clone();
                    trace.seq_assign(exit.trace.clone());
                    exit.trace = trace;
                }
                result.exits.join_assign(&handler_state.exits);
                let mut summary = handler_state.summary;
                summary.seq_assign(&result.summary);
                result.summary = summary;
                control.join_normal(result);
                control
            }
        }
    }

    pub(crate) fn materialize_handler_value(
        &mut self,
        expr: HirExprId,
        span: Span,
        handlers: &[HirHandlerArmId],
        mut state: EffectState,
    ) -> EffectState {
        let Some(rows) = self.checked_handler_rows(expr, span) else {
            return self.incomplete_at(
                span,
                "handler value requires a checked handler type fact",
                state,
            );
        };
        let Some(owner) = state.owner else {
            return self.incomplete_at(
                span,
                "handler value requires an owning effect analysis unit",
                state,
            );
        };
        let mut inferred_produced = EffectRow::empty();
        let mut arm_facts = Vec::new();
        let mut unsupported = false;
        let captures = match self.lambda_value_captures(expr, &state) {
            Ok(captures) => captures,
            Err(reason) => return self.incomplete_at(span, reason, state),
        };

        for arm in handlers {
            self.discovered_value_captures
                .entry(EffectUnit::HandlerArm { owner, arm: *arm })
                .or_default()
                .join_assign(&captures);
            let Some(arm_data) = self.hir.handler_arms.get(*arm).cloned() else {
                state = self.incomplete_at(
                    span,
                    "handler value requires materialized handler arm facts",
                    state,
                );
                unsupported = true;
                continue;
            };
            let Some((effect, effect_type_args)) =
                self.materialize_handler_action_effect(*arm, &arm_data.action, arm_data.span)
            else {
                unsupported = true;
                continue;
            };
            let arm_summary = self
                .inputs
                .unit_effects
                .get(&crate::EffectUnit::HandlerArm { owner, arm: *arm })
                .cloned();
            let Some(arm_summary) = arm_summary else {
                state = self.incomplete_at(
                    arm_data.span,
                    "handler value requires solved handler arm effect facts",
                    state,
                );
                unsupported = true;
                continue;
            };
            state
                .summary
                .merge_request_footprint_without_escaping_or_trace(&arm_summary);
            inferred_produced.union_assign(&arm_summary.escaping_effects);
            if matches!(
                arm_summary.support,
                InterpreterSupport::Rejected(FrontendRejectionReason::UnsupportedHandler)
            ) {
                unsupported = true;
            }

            let resume_scan = self.scan_handler_controls_in_block(arm_data.body);
            if resume_scan.resumes > 1 {
                self.diagnostics.push(Diagnostic::effect_check(
                    EffectDiagnosticCode::ResumeUsedMoreThanOnce,
                    arm_data.span,
                    "handler arm may resume at most once",
                ));
            }
            if resume_scan.captured_by_lambda {
                self.diagnostics.push(Diagnostic::effect_check(
                    EffectDiagnosticCode::InvalidResume,
                    arm_data.span,
                    "resume cannot be captured by an anonymous flow",
                ));
            }

            let returns_never = self
                .action_signature_for_arm(&arm_data.action)
                .is_some_and(|signature| signature.returns_never);
            if returns_never && resume_scan.resumes > 0 {
                self.diagnostics.push(Diagnostic::effect_check(
                    EffectDiagnosticCode::CannotResumeNeverAction,
                    arm_data.span,
                    "cannot resume an action whose return type is never",
                ));
            }
            let resumes = match resume_scan.resumes {
                0 => ResumeSummary::None,
                1 => ResumeSummary::Once,
                _ => ResumeSummary::Multiple,
            };
            let finishes = match resume_scan.finishes {
                0 => crate::FinishSummary::None,
                1 => crate::FinishSummary::Once,
                _ => crate::FinishSummary::Multiple,
            };
            self.inputs
                .handler_action_effects
                .insert(*arm, effect.clone());
            arm_facts.push(HandlerArmFact {
                unit: EffectUnit::HandlerArm { owner, arm: *arm },
                action: HandlerActionFact {
                    effect_segments: arm_data
                        .action
                        .effect
                        .path
                        .segments
                        .iter()
                        .map(|segment| segment.name.clone())
                        .collect(),
                    action: arm_data.action.action.clone(),
                    action_symbol: match arm_data.action.action_symbol {
                        etas_hir::ResolveResult::Resolved(symbol) => Some(symbol),
                        _ => None,
                    },
                    effect_type_args,
                    span: arm_data.span,
                },
                resumable: !returns_never,
                resumes,
                completion: HandlerCompletionSummary {
                    resumes,
                    finishes,
                    terminates_never: self.block_type_is_never(arm_data.body),
                },
                arm_effects: arm_summary.escaping_effects,
            });

            let expected_effect = EffectRow::closed(EffectSet::one(effect));
            let coverage = EffectCoverage {
                registry: self.registry,
                types: &self.types.store,
            };
            if !coverage.row_covers(&rows.handled, &expected_effect) {
                self.diagnostics.push(Diagnostic::effect_check(
                    EffectDiagnosticCode::HandlerArmOutsideHandledRow,
                    arm_data.span,
                    "handler value handled row does not cover a materialized handler arm action",
                ));
                unsupported = true;
            }
        }

        let produced = match rows.produced {
            Some(explicit) => {
                let coverage = EffectCoverage {
                    registry: self.registry,
                    types: &self.types.store,
                };
                if !coverage.row_covers(&explicit, &inferred_produced) {
                    self.diagnostics.push(Diagnostic::effect_check(
                        EffectDiagnosticCode::HandlerProducedEffectOutsideDeclaredRow,
                        span,
                        "handler arm produced effects escape the declared produced row",
                    ));
                    unsupported = true;
                }
                if inferred_produced.effects.is_empty() {
                    explicit
                } else {
                    inferred_produced
                }
            }
            None => inferred_produced,
        };

        if unsupported {
            state.summary.support =
                InterpreterSupport::Rejected(FrontendRejectionReason::UnsupportedHandler);
        }
        self.inputs.handler_values.insert(
            expr,
            HandlerValueFact {
                handled: rows.handled,
                produced,
                result: rows.result,
                arms: arm_facts,
            },
        );
        state
    }

    fn handler_rows_for_expr(
        &mut self,
        expr: HirExprId,
        span: Span,
    ) -> Result<Option<(EffectRow, EffectRow)>, etas_utils::PipelineInterruption> {
        if let Some(fact) = self.inputs.handler_values.get(&expr) {
            return Ok(Some((fact.handled.clone(), fact.produced.clone())));
        }
        let Some(rows) = self.checked_handler_rows(expr, span) else {
            return Ok(None);
        };
        if let Some(produced) = rows.produced {
            return Ok(Some((rows.handled, produced)));
        }
        let Some((_, fact)) = self.immutable_handler_value(expr)? else {
            return Ok(None);
        };
        let coverage = EffectCoverage {
            registry: self.registry,
            types: &self.types.store,
        };
        if !coverage.row_covers(&rows.handled, &fact.handled)
            || !coverage.row_covers(&fact.handled, &rows.handled)
        {
            return Ok(None);
        }
        Ok(Some((rows.handled, fact.produced.clone())))
    }

    fn immutable_handler_value(
        &self,
        mut expr: HirExprId,
    ) -> Result<Option<(HirExprId, &HandlerValueFact)>, etas_utils::PipelineInterruption> {
        let mut visited = std::collections::HashSet::new();
        while visited.insert(expr) {
            if let Some(control) = self.query_control {
                control.checkpoint()?;
            }
            if let Some(fact) = self.inputs.handler_values.get(&expr) {
                return Ok(Some((expr, fact)));
            }
            let Some(HirExpr::Path(path)) = self.hir.exprs.get(expr) else {
                return Ok(None);
            };
            let etas_hir::ResolveResult::Resolved(symbol) = path.resolution else {
                return Ok(None);
            };
            let Some(symbol) = self.type_symbols.canonical_symbol(self.hir, symbol) else {
                return Ok(None);
            };
            expr = match self.hir.symbols.get(symbol).map(|symbol| &symbol.def) {
                Some(etas_hir::SymbolDef::TopLevelLet { initializer, .. }) => *initializer,
                Some(etas_hir::SymbolDef::Local {
                    binding,
                    pattern,
                    initializer: Some(initializer),
                    ..
                }) if matches!(self.hir.stmts.get(*binding), Some(HirStmt::Let { pat, value, .. }) if pat == pattern && value == initializer)
                    && matches!(self.hir.pats.get(*pattern), Some(etas_hir::HirPat::Binding { symbol: bound, .. }) if *bound == symbol) =>
                {
                    *initializer
                }
                _ => return Ok(None),
            };
        }
        Ok(None)
    }

    pub(super) fn definitely_handled_actions(
        &self,
        expr: HirExprId,
    ) -> Result<EffectRow, etas_utils::PipelineInterruption> {
        let Some((handler, fact)) = self.immutable_handler_value(expr)? else {
            return Ok(EffectRow::empty());
        };
        let Some(HirExpr::Handler { handlers, .. }) = self.hir.exprs.get(handler) else {
            return Ok(EffectRow::empty());
        };
        let Some(owner) = self.hir.index().expr_item.get(&handler).copied() else {
            return Ok(EffectRow::empty());
        };
        if handlers.len() != fact.arms.len() {
            return Ok(EffectRow::empty());
        }
        let mut effects = EffectSet::new();
        for (arm, arm_fact) in handlers.iter().zip(&fact.arms) {
            if let Some(control) = self.query_control {
                control.checkpoint()?;
            }
            if arm_fact.unit != (EffectUnit::HandlerArm { owner, arm: *arm }) {
                return Ok(EffectRow::empty());
            }
            let (Some(data), Some(effect)) = (
                self.hir.handler_arms.get(*arm),
                self.inputs.handler_action_effects.get(arm),
            ) else {
                return Ok(EffectRow::empty());
            };
            if data.patterns.iter().all(|pattern| {
                matches!(
                    self.hir.pats.get(*pattern),
                    Some(etas_hir::HirPat::Binding { .. } | etas_hir::HirPat::Wildcard { .. })
                )
            }) {
                effects.insert(effect.clone());
            }
        }
        Ok(EffectRow::closed(effects))
    }

    fn handler_value_item_summary(&self, expr: HirExprId) -> Option<EffectSummary> {
        let HirExpr::Path(path) = self.hir.exprs.get(expr)? else {
            return None;
        };
        let etas_hir::ResolveResult::Resolved(symbol) = path.resolution else {
            return None;
        };
        let item = self.source_item_for_symbol(symbol)?;
        if !matches!(
            self.hir.items.get(item),
            Some(etas_hir::HirItem::TopLevelLet(_))
        ) {
            return None;
        }
        self.inputs
            .unit_effects
            .get(&EffectUnit::Item(item))
            .cloned()
    }

    fn checked_handler_rows(&self, expr: HirExprId, _span: Span) -> Option<CheckedHandlerRows> {
        let ty = self.types.facts.expr_types.get(&expr)?;
        let Type::Handler(handler) = self.types.store.get(*ty)?.clone() else {
            return None;
        };
        let handled = self.row_from_type_ref(&handler.handled);
        let produced = match handler.produced {
            HandlerProducedEffects::Explicit(row) => Some(self.row_from_type_ref(&row)),
            HandlerProducedEffects::Infer => None,
        };
        Some(CheckedHandlerRows {
            handled,
            produced,
            result: handler.result,
        })
    }

    fn materialize_handler_action_effect(
        &mut self,
        arm: HirHandlerArmId,
        action: &etas_hir::ResolvedActionRef,
        span: Span,
    ) -> Option<(crate::Effect, Vec<TypeId>)> {
        for arg in &action.effect.args {
            if self.type_arg_from_hir_effect_arg(arg).is_none() {
                self.diagnostics.push(Diagnostic::effect_check(
                    EffectDiagnosticCode::IncompleteEffectFacts,
                    span,
                    "handler arm effect solving requires checked handled-action type argument facts",
                ));
                return None;
            }
        }
        let Some(checked) = self.types.facts.handler_actions.get(&arm) else {
            self.diagnostics.push(Diagnostic::effect_check(
                EffectDiagnosticCode::IncompleteEffectFacts,
                span,
                "handler action requires checked selector and signature facts",
            ));
            return None;
        };
        let effect = self.effect_from_action_ref(action).or_else(|| {
            self.diagnostics.push(Diagnostic::effect_check(
                EffectDiagnosticCode::IncompleteEffectFacts,
                span,
                "handler arm effect solving requires a checked action signature fact",
            ));
            None
        })?;
        let effect_type_args = checked
            .selectors
            .iter()
            .filter_map(|arg| match arg {
                etas_types::EffectArgRef::Type(ty) => Some(*ty),
                _ => None,
            })
            .collect();
        let effect = match effect {
            crate::Effect::Action(action) if !checked.selectors.is_empty() => {
                crate::Effect::AppliedAction(crate::ActionInstanceRef {
                    action,
                    args: checked.selectors.clone(),
                })
            }
            crate::Effect::AppliedAction(mut instance) => {
                instance.args = checked.selectors.clone();
                crate::Effect::AppliedAction(instance)
            }
            effect => effect,
        };
        Some((effect, effect_type_args))
    }

    fn remove_handled_runtime_support(&self, summary: &mut EffectSummary, handled: &EffectRow) {
        let removed = self.host_requirements_for_row(handled);
        if removed.is_empty() {
            return;
        }
        summary.support = summary.support.without_host_requirements(&removed);
    }

    fn host_requirements_for_row(&self, row: &EffectRow) -> Vec<HostRequirementKind> {
        let mut requirements = Vec::new();
        for effect in row.effects.iter() {
            let reason = match effect {
                crate::Effect::Action(action) => self
                    .registry
                    .runtime_requirement_reason_for_action_ref(action),
                crate::Effect::AppliedAction(action) => self
                    .registry
                    .runtime_requirement_reason_for_action_ref(&action.action),
                crate::Effect::Tag(tag) | crate::Effect::Applied { tag, .. } => {
                    self.registry.runtime_requirement_reason(*tag)
                }
                crate::Effect::Error(_) | crate::Effect::Var(_) => None,
            };
            if let Some(kind) = reason.and_then(|reason| reason.host_requirement_kind()) {
                requirements.push(kind);
            }
        }
        requirements.sort();
        requirements.dedup();
        requirements
    }

    fn action_signature_for_arm(
        &self,
        action: &etas_hir::ResolvedActionRef,
    ) -> Option<&crate::EffectActionSig> {
        self.signature_for_action_ref(action)
    }

    fn block_type_is_never(&self, block: HirBlockId) -> bool {
        let Some(block) = self.hir.blocks.get(block) else {
            return false;
        };
        let ty = block
            .final_expr
            .and_then(|expr| self.types.facts.expr_types.get(&expr).copied())
            .or_else(|| {
                block
                    .stmts
                    .last()
                    .and_then(|stmt| self.types.facts.stmt_types.get(stmt).copied())
            });
        ty.is_some_and(|ty| {
            matches!(
                self.types.store.get(ty),
                Some(Type::Primitive(PrimitiveType::Never))
            )
        })
    }

    fn scan_handler_controls_in_block(&self, block: HirBlockId) -> HandlerControlScan {
        let Some(block) = self.hir.blocks.get(block) else {
            return HandlerControlScan::default();
        };
        let mut scan = HandlerControlScan::default();
        for stmt in &block.stmts {
            self.scan_handler_controls_in_stmt(*stmt, false, &mut scan);
        }
        if let Some(expr) = block.final_expr {
            self.scan_handler_controls_in_expr(expr, false, &mut scan);
        }
        scan
    }

    fn scan_handler_controls_in_stmt(
        &self,
        stmt: etas_hir::HirStmtId,
        in_lambda: bool,
        scan: &mut HandlerControlScan,
    ) {
        let Some(stmt) = self.hir.stmts.get(stmt) else {
            return;
        };
        match stmt {
            HirStmt::Resume { .. } if in_lambda => scan.captured_by_lambda = true,
            HirStmt::Resume { .. } => scan.resumes += 1,
            HirStmt::Finish { value, .. } if in_lambda => {
                scan.finish_captured_by_lambda = true;
                self.scan_handler_controls_in_expr(*value, in_lambda, scan);
            }
            HirStmt::Finish { value, .. } => {
                scan.finishes += 1;
                self.scan_handler_controls_in_expr(*value, in_lambda, scan);
            }
            HirStmt::Let { value, .. } | HirStmt::Var { value, .. } => {
                self.scan_handler_controls_in_expr(*value, in_lambda, scan)
            }
            HirStmt::Assign { target, value, .. } => {
                self.scan_handler_controls_in_expr(*target, in_lambda, scan);
                self.scan_handler_controls_in_expr(*value, in_lambda, scan);
            }
            HirStmt::If(expr) | HirStmt::Match(expr) | HirStmt::Expr { expr, .. } => {
                self.scan_handler_controls_in_expr(*expr, in_lambda, scan)
            }
            HirStmt::For { iter, body, .. } => {
                self.scan_handler_controls_in_expr(*iter, in_lambda, scan);
                self.scan_handler_controls_in_block_with_context(*body, in_lambda, scan);
            }
            HirStmt::While { cond, body, .. } => {
                self.scan_handler_controls_in_expr(*cond, in_lambda, scan);
                self.scan_handler_controls_in_block_with_context(*body, in_lambda, scan);
            }
            HirStmt::Retry { body, .. } => {
                self.scan_handler_controls_in_block_with_context(*body, in_lambda, scan);
            }
            HirStmt::Return { value, .. } => {
                if let Some(value) = value {
                    self.scan_handler_controls_in_expr(*value, in_lambda, scan);
                }
            }
            HirStmt::Break { .. } | HirStmt::Continue { .. } | HirStmt::Error { .. } => {}
        }
    }

    fn scan_handler_controls_in_block_with_context(
        &self,
        block: HirBlockId,
        in_lambda: bool,
        scan: &mut HandlerControlScan,
    ) {
        let Some(block) = self.hir.blocks.get(block) else {
            return;
        };
        for stmt in &block.stmts {
            self.scan_handler_controls_in_stmt(*stmt, in_lambda, scan);
        }
        if let Some(expr) = block.final_expr {
            self.scan_handler_controls_in_expr(expr, in_lambda, scan);
        }
    }

    fn scan_handler_controls_in_expr(
        &self,
        expr: HirExprId,
        in_lambda: bool,
        scan: &mut HandlerControlScan,
    ) {
        let Some(expr) = self.hir.exprs.get(expr) else {
            return;
        };
        match expr {
            HirExpr::Lambda { body, .. } => match body {
                etas_hir::HirLambdaBody::Expr(expr) => {
                    self.scan_handler_controls_in_expr(*expr, true, scan);
                }
                etas_hir::HirLambdaBody::Block(block) => {
                    self.scan_handler_controls_in_block_with_context(*block, true, scan);
                }
            },
            HirExpr::Block(block) => {
                self.scan_handler_controls_in_block_with_context(*block, in_lambda, scan)
            }
            HirExpr::If {
                cond,
                then_block,
                else_branch,
                ..
            } => {
                self.scan_handler_controls_in_expr(*cond, in_lambda, scan);
                self.scan_handler_controls_in_block_with_context(*then_block, in_lambda, scan);
                match else_branch {
                    Some(etas_hir::HirElseBranch::If(expr)) => {
                        self.scan_handler_controls_in_expr(*expr, in_lambda, scan);
                    }
                    Some(etas_hir::HirElseBranch::Block(block)) => {
                        self.scan_handler_controls_in_block_with_context(*block, in_lambda, scan);
                    }
                    None => {}
                }
            }
            HirExpr::Match {
                scrutinee, arms, ..
            } => {
                self.scan_handler_controls_in_expr(*scrutinee, in_lambda, scan);
                for arm in arms {
                    match arm.body {
                        etas_hir::HirMatchArmBody::Expr(expr) => {
                            self.scan_handler_controls_in_expr(expr, in_lambda, scan);
                        }
                        etas_hir::HirMatchArmBody::Block(block) => {
                            self.scan_handler_controls_in_block_with_context(
                                block, in_lambda, scan,
                            );
                        }
                    }
                }
            }
            HirExpr::Call { callee, args, .. } => {
                self.scan_handler_controls_in_expr(*callee, in_lambda, scan);
                for arg in args {
                    self.scan_handler_controls_in_arg(arg, in_lambda, scan);
                }
            }
            HirExpr::MethodCall { receiver, args, .. } => {
                self.scan_handler_controls_in_expr(*receiver, in_lambda, scan);
                for arg in args {
                    self.scan_handler_controls_in_arg(arg, in_lambda, scan);
                }
            }
            HirExpr::SpecMethodCall { receiver, args, .. } => {
                self.scan_handler_controls_in_expr(*receiver, in_lambda, scan);
                for arg in args {
                    self.scan_handler_controls_in_arg(arg, in_lambda, scan);
                }
            }
            HirExpr::Perform { args, .. } | HirExpr::Infer { args, .. } => {
                for arg in args {
                    self.scan_handler_controls_in_arg(arg, in_lambda, scan);
                }
            }
            HirExpr::Handler { handlers, .. } => {
                for arm in handlers {
                    if let Some(arm) = self.hir.handler_arms.get(*arm) {
                        self.scan_handler_controls_in_block_with_context(arm.body, in_lambda, scan);
                    }
                }
            }
            HirExpr::Handle { body, handler, .. } => {
                self.scan_handler_controls_in_expr(*body, in_lambda, scan);
                self.scan_handler_controls_in_expr(*handler, in_lambda, scan);
            }
            HirExpr::StageCompose { stages, .. } => {
                for stage in stages {
                    self.scan_handler_controls_in_expr(stage.expr, in_lambda, scan);
                    for limit in &stage.limits {
                        self.scan_handler_controls_in_expr(*limit, in_lambda, scan);
                    }
                }
            }
            HirExpr::Pipeline { input, stages, .. } => {
                self.scan_handler_controls_in_expr(*input, in_lambda, scan);
                for stage in stages {
                    self.scan_handler_controls_in_expr(stage.expr, in_lambda, scan);
                    for limit in &stage.limits {
                        self.scan_handler_controls_in_expr(*limit, in_lambda, scan);
                    }
                }
            }
            HirExpr::Record(record) => {
                for field in &record.fields {
                    self.scan_handler_controls_in_field(field, in_lambda, scan);
                }
            }
            HirExpr::Tuple { elems, .. }
            | HirExpr::Array { elems, .. }
            | HirExpr::List { elems, .. }
            | HirExpr::Set { elems, .. } => {
                for elem in elems {
                    self.scan_handler_controls_in_expr(*elem, in_lambda, scan);
                }
            }
            HirExpr::Map { entries, .. } => {
                for entry in entries {
                    self.scan_handler_controls_in_expr(entry.key, in_lambda, scan);
                    self.scan_handler_controls_in_expr(entry.value, in_lambda, scan);
                }
            }
            HirExpr::ListCons { head, tail, .. }
            | HirExpr::Range {
                start: head,
                end: tail,
                ..
            }
            | HirExpr::Binary {
                lhs: head,
                rhs: tail,
                ..
            } => {
                self.scan_handler_controls_in_expr(*head, in_lambda, scan);
                self.scan_handler_controls_in_expr(*tail, in_lambda, scan);
            }
            HirExpr::Field { base, .. } | HirExpr::Unary { expr: base, .. } => {
                self.scan_handler_controls_in_expr(*base, in_lambda, scan);
            }
            HirExpr::Try { expr, .. } => self.scan_handler_controls_in_expr(*expr, in_lambda, scan),
            HirExpr::Index { base, index, .. } => {
                self.scan_handler_controls_in_expr(*base, in_lambda, scan);
                self.scan_handler_controls_in_expr(*index, in_lambda, scan);
            }
            HirExpr::Slice {
                base, start, end, ..
            } => {
                self.scan_handler_controls_in_expr(*base, in_lambda, scan);
                self.scan_handler_controls_in_expr(*start, in_lambda, scan);
                self.scan_handler_controls_in_expr(*end, in_lambda, scan);
            }
            HirExpr::Literal(_)
            | HirExpr::Path(_)
            | HirExpr::Reflect(_)
            | HirExpr::EmptyRecordOrMap { .. }
            | HirExpr::EmptySequence { .. }
            | HirExpr::Error { .. } => {}
        }
    }

    fn scan_handler_controls_in_arg(
        &self,
        arg: &etas_hir::HirArg,
        in_lambda: bool,
        scan: &mut HandlerControlScan,
    ) {
        match arg {
            etas_hir::HirArg::Positional(expr) | etas_hir::HirArg::Named { value: expr, .. } => {
                self.scan_handler_controls_in_expr(*expr, in_lambda, scan);
            }
        }
    }

    fn scan_handler_controls_in_field(
        &self,
        field: &etas_hir::HirFieldInit,
        in_lambda: bool,
        scan: &mut HandlerControlScan,
    ) {
        if let etas_hir::HirFieldInit::Named { value, .. } = field {
            self.scan_handler_controls_in_expr(*value, in_lambda, scan);
        }
    }
}

struct CheckedHandlerRows {
    handled: EffectRow,
    produced: Option<EffectRow>,
    result: Option<etas_types::TypeId>,
}

#[derive(Default)]
struct HandlerControlScan {
    resumes: usize,
    finishes: usize,
    captured_by_lambda: bool,
    finish_captured_by_lambda: bool,
}

#[cfg(test)]
mod inert_tests {
    use super::*;

    #[test]
    fn handler_construction_must_not_add_actions_effects_or_exit_gaps() {
        let entry = EffectState::default();
        assert!(handler_evaluation_is_inert(&entry, &entry, &entry.summary));
        let action = crate::Effect::Action(crate::ActionRef {
            tag: crate::CONSOLE_TAG,
            action: crate::CONSOLE_STDOUT_WRITE_ACTION,
        });
        let mut latent = EffectSummary::local();
        latent.record_requested_action(action.clone());
        let mut expected = entry.summary.clone();
        expected.merge_request_footprint_without_escaping_or_trace(&latent);
        let mut latent_handler = entry.clone();
        latent_handler.summary = expected.clone();
        assert!(handler_evaluation_is_inert(
            &entry,
            &latent_handler,
            &expected
        ));
        latent_handler.summary.action_trace =
            crate::ActionTraceDomain::UnknownOrder([action.clone()].into_iter().collect());
        assert!(!handler_evaluation_is_inert(
            &entry,
            &latent_handler,
            &expected
        ));
        let mut handler = entry.clone();
        handler.summary.action_trace =
            crate::ActionTraceDomain::UnknownOrder([action.clone()].into_iter().collect());
        assert!(!handler_evaluation_is_inert(
            &entry,
            &handler,
            &entry.summary
        ));
        handler = entry.clone();
        handler.summary.record_escaping_effect(action);
        assert!(!handler_evaluation_is_inert(
            &entry,
            &handler,
            &entry.summary
        ));
        handler = entry.clone();
        handler.summary.determinism = crate::Determinism::NonDeterministic;
        assert!(!handler_evaluation_is_inert(
            &entry,
            &handler,
            &entry.summary
        ));
        handler = entry.clone();
        handler.summary.independent_determinism = None;
        assert!(!handler_evaluation_is_inert(
            &entry,
            &handler,
            &entry.summary
        ));
        handler = entry.clone();
        handler
            .summary
            .require_runtime(crate::RuntimeRequirementReason::Time);
        assert!(!handler_evaluation_is_inert(
            &entry,
            &handler,
            &entry.summary
        ));
        handler = entry.clone();
        handler
            .exits
            .gaps
            .insert(crate::OutcomeAnalysisGap::InvalidControlExit);
        assert!(!handler_evaluation_is_inert(
            &entry,
            &handler,
            &entry.summary
        ));
        handler = entry.clone();
        handler.incomplete = true;
        assert!(!handler_evaluation_is_inert(
            &entry,
            &handler,
            &entry.summary
        ));
    }
}
