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

impl EffectSemantics<'_> {
    pub(crate) fn handle_parts(
        &mut self,
        expr: HirExprId,
        span: Span,
        parts: HandleParts<EffectState>,
    ) -> Control<EffectState> {
        match parts.handler {
            HandlerParts::Expr {
                expr: handler,
                control,
            } => {
                let body_state = parts.body.into_joined_domain();
                let mut handler_state = control.into_joined_domain();
                if let Some(summary) = self.handler_value_item_summary(handler) {
                    handler_state
                        .summary
                        .merge_possible_handler_footprint_without_trace(&summary);
                }
                let Some((handled, produced)) = self.handler_rows_for_expr(handler, span) else {
                    return Control::normal(body_state);
                };
                let coverage = EffectCoverage {
                    registry: self.registry,
                    types: &self.types.store,
                };
                let remaining =
                    coverage.subtract_handled(&body_state.summary.escaping_effects, &handled);
                let mut result = body_state;
                result.summary.escaping_effects = remaining.clone();
                result.summary.default_actions =
                    coverage.subtract_handled(&result.summary.default_actions, &handled);
                result.summary.record_handled_action_row(handled.clone());
                self.remove_handled_runtime_support(&mut result.summary, &handled);
                self.apply_runtime_support_for_row(&mut result.summary, &remaining);
                result
                    .summary
                    .merge_possible_handler_footprint_without_trace(&handler_state.summary);
                result.summary.escaping_effects.union_assign(&produced);
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
                Control::normal(result)
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

        for arm in handlers {
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
                self.materialize_handler_action_effect(&arm_data.action, arm_data.span)
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
                .merge_possible_handler_footprint_without_trace(&arm_summary);
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
            arm_facts.push(HandlerArmFact {
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
    ) -> Option<(EffectRow, EffectRow)> {
        if let Some(fact) = self.inputs.handler_values.get(&expr) {
            return Some((fact.handled.clone(), fact.produced.clone()));
        }
        let rows = self.checked_handler_rows(expr, span)?;
        let produced = rows.produced.unwrap_or_else(EffectRow::empty);
        Some((rows.handled, produced))
    }

    fn handler_value_item_summary(&self, expr: HirExprId) -> Option<EffectSummary> {
        let HirExpr::Path(path) = self.hir.exprs.get(expr)? else {
            return None;
        };
        let etas_hir::ResolveResult::Resolved(symbol) = path.resolution else {
            return None;
        };
        let symbol = self.hir.symbols.get(symbol)?;
        let item = match &symbol.def {
            etas_hir::SymbolDef::Item { item } | etas_hir::SymbolDef::TopLevelLet { item, .. } => {
                *item
            }
            etas_hir::SymbolDef::ImportAlias {
                path,
                origin: etas_hir::ImportAliasOrigin::SourceImport,
            } => self.source_item_for_path(path)?,
            etas_hir::SymbolDef::ImportAlias { .. } => return None,
            _ => return None,
        };
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
        action: &etas_hir::ResolvedActionRef,
        span: Span,
    ) -> Option<(crate::Effect, Vec<TypeId>)> {
        let effect = self.effect_from_action_ref(action).or_else(|| {
            self.diagnostics.push(Diagnostic::effect_check(
                EffectDiagnosticCode::IncompleteEffectFacts,
                span,
                "handler arm effect solving requires a checked action signature fact",
            ));
            None
        })?;
        let mut effect_type_args = Vec::new();
        for owner_arg in &action.effect.args {
            let Some(type_arg) = self.type_arg_from_hir_effect_arg(owner_arg) else {
                self.diagnostics.push(Diagnostic::effect_check(
                    EffectDiagnosticCode::IncompleteEffectFacts,
                    span,
                    "handler arm effect solving requires checked handled-action type argument facts",
                ));
                return None;
            };
            effect_type_args.push(type_arg);
        }
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
            HirExpr::Perform { args, .. } => {
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
