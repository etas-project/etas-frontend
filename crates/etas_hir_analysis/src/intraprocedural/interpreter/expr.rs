use etas_hir::{HirArg, HirElseBranch, HirExpr, HirExprId, HirFieldInit, HirMatchArmBody};

use super::Traversal;
use crate::intraprocedural::{Control, HandleParts, HandlerParts, HirAnalysisSemantics};

impl<S, F, E> Traversal<'_, S, F>
where
    S: HirAnalysisSemantics,
    F: Fn() -> Result<(), E>,
{
    pub(super) fn expr_control(
        &mut self,
        expr: HirExprId,
        state: S::Domain,
    ) -> Result<Control<S::Domain>, E> {
        (self.checkpoint)()?;
        let state = self.semantics.before_expr(expr, state);
        let expr_data = self.context().program().exprs[expr].clone();
        let span = expr_data.span(&self.context().program().blocks);
        let next = match expr_data {
            HirExpr::Infer { args, .. } => self.args_control(&args, Control::normal(state))?,
            HirExpr::Perform {
                action,
                generic_args,
                args,
                ..
            } => {
                let control = self.args_control(&args, Control::normal(state))?;
                self.apply_normal_step(control, |this, state| {
                    this.semantics
                        .perform(expr, &action, &generic_args, &args, span, state)
                })?
            }
            HirExpr::If { .. } => self.if_chain_control(expr, state)?,
            HirExpr::Match {
                scrutinee, arms, ..
            } => {
                let control = self.expr_control(scrutinee, state)?;
                control.try_with_normal_processed(|base| {
                    let mut joined = Control::bottom();
                    for arm in arms {
                        (self.checkpoint)()?;
                        let arm_state = match arm.body {
                            HirMatchArmBody::Expr(expr) => self.expr_control(expr, base.clone())?,
                            HirMatchArmBody::Block(block) => {
                                self.block_control(block, base.clone())?
                            }
                        };
                        joined.join_assign(&arm_state);
                    }
                    Ok(joined)
                })?
            }
            HirExpr::Block(block) => self.block_control(block, state)?,
            HirExpr::Call { callee, args, .. } => {
                let control = self.expr_control(callee, state)?;
                let control = self.args_control(&args, control)?;
                control.try_with_normal_processed(|state| {
                    self.direct_call_control(expr, callee, span, state)
                })?
            }
            HirExpr::MethodCall { .. } => self.method_call_chain_control(expr, state)?,
            HirExpr::SpecMethodCall { receiver, args, .. } => {
                let control = self.expr_control(receiver, state)?;
                self.args_control(&args, control)?
            }
            HirExpr::Record(record) => {
                self.record_fields_control(record.fields, Control::normal(state))?
            }
            HirExpr::Tuple { elems, .. }
            | HirExpr::Array { elems, .. }
            | HirExpr::List { elems, .. }
            | HirExpr::Set { elems, .. } => self.exprs_control(elems, Control::normal(state))?,
            HirExpr::Map { entries, .. } => {
                entries
                    .into_iter()
                    .try_fold(Control::normal(state), |control, entry| {
                        (self.checkpoint)()?;
                        let control = control.try_with_normal_processed(|state| {
                            self.expr_control(entry.key, state)
                        })?;
                        control.try_with_normal_processed(|state| {
                            self.expr_control(entry.value, state)
                        })
                    })?
            }
            HirExpr::ListCons { .. }
            | HirExpr::Range { .. }
            | HirExpr::Binary { .. }
            | HirExpr::Field { .. }
            | HirExpr::Unary { .. }
            | HirExpr::Index { .. }
            | HirExpr::Slice { .. } => self.structural_expr_control(expr, state)?,
            HirExpr::Try { expr: base, .. } => {
                let entry = state.clone();
                let operand = self.semantics.begin_try_operand(expr, state);
                let control = self.expr_control(base, operand)?;
                (self.checkpoint)()?;
                self.semantics.try_expr_control(expr, span, entry, control)
            }
            HirExpr::Handle { body, handler, .. } => {
                let entry = state.clone();
                let body_state = self.semantics.begin_handle_body(expr, state);
                let body_control = self.expr_control(body, body_state)?;
                let handler_parts = HandlerParts::Expr {
                    expr: handler,
                    control: self.expr_control(handler, entry.clone())?,
                };
                (self.checkpoint)()?;
                self.semantics.handle_expr_control(
                    expr,
                    span,
                    entry,
                    HandleParts {
                        body: body_control,
                        handler: handler_parts,
                    },
                )
            }
            HirExpr::StageCompose { stages, .. } => {
                if self.semantics.stage_targets_are_captured(expr, &state) {
                    let control = self
                        .apply_normal_step(Control::normal(state), |this, state| {
                            this.semantics.stage_compose(expr, span, state)
                        })?;
                    return Ok(self.semantics.after_expr_control(expr, control));
                }
                let control =
                    stages
                        .into_iter()
                        .try_fold(Control::normal(state), |control, stage| {
                            (self.checkpoint)()?;
                            let control = control.try_with_normal_processed(|state| {
                                self.expr_control(stage.expr, state)
                            })?;
                            self.exprs_control(stage.limits, control)
                        })?;
                self.apply_normal_step(control, |this, state| {
                    this.semantics.stage_compose(expr, span, state)
                })?
            }
            HirExpr::Pipeline { input, stages, .. } => {
                let control = self.expr_control(input, state)?;
                let control = stages.into_iter().try_fold(control, |control, stage| {
                    (self.checkpoint)()?;
                    let control = control
                        .try_with_normal_processed(|state| self.expr_control(stage.expr, state))?;
                    self.exprs_control(stage.limits, control)
                })?;
                self.apply_normal_step(control, |this, state| {
                    this.semantics.pipeline(expr, span, state)
                })?
            }
            HirExpr::Lambda { .. } => {
                let step = self.semantics.lambda_boundary(expr, span, state);
                Control::normal(self.require_handled(step))
            }
            HirExpr::Handler { .. } => {
                let step = self.semantics.handler_boundary(expr, span, state);
                Control::normal(self.require_handled(step))
            }
            HirExpr::Literal(_)
            | HirExpr::Path(_)
            | HirExpr::Reflect(_)
            | HirExpr::EmptyRecordOrMap { .. }
            | HirExpr::EmptySequence { .. }
            | HirExpr::Error { .. } => Control::normal(state),
        };
        (self.checkpoint)()?;
        Ok(self.semantics.after_expr_control(expr, next))
    }

    pub(crate) fn args_control(
        &mut self,
        args: &[HirArg],
        control: Control<S::Domain>,
    ) -> Result<Control<S::Domain>, E> {
        args.iter().try_fold(control, |control, arg| match arg {
            HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => {
                (self.checkpoint)()?;
                control.try_with_normal_processed(|state| self.expr_control(*expr, state))
            }
        })
    }

    pub(crate) fn exprs_control(
        &mut self,
        exprs: impl IntoIterator<Item = HirExprId>,
        control: Control<S::Domain>,
    ) -> Result<Control<S::Domain>, E> {
        exprs.into_iter().try_fold(control, |control, expr| {
            (self.checkpoint)()?;
            control.try_with_normal_processed(|state| self.expr_control(expr, state))
        })
    }

    fn record_fields_control(
        &mut self,
        fields: impl IntoIterator<Item = HirFieldInit>,
        control: Control<S::Domain>,
    ) -> Result<Control<S::Domain>, E> {
        fields.into_iter().try_fold(control, |control, field| {
            (self.checkpoint)()?;
            match field {
                HirFieldInit::Named { value, .. } => {
                    control.try_with_normal_processed(|state| self.expr_control(value, state))
                }
                HirFieldInit::Shorthand { .. } => Ok(control),
            }
        })
    }

    fn method_call_chain_control(
        &mut self,
        expr: HirExprId,
        mut state: S::Domain,
    ) -> Result<Control<S::Domain>, E> {
        let mut chain = Vec::new();
        let mut current = expr;
        while let HirExpr::MethodCall { receiver, .. } = &self.context().program().exprs[current] {
            (self.checkpoint)()?;
            let receiver = *receiver;
            if current != expr {
                state = self.semantics.before_expr(current, state);
            }
            chain.push(current);
            current = receiver;
        }

        let mut control = self.expr_control(current, state)?;
        for call in chain.into_iter().rev() {
            (self.checkpoint)()?;
            let (args, span) = match &self.context().program().exprs[call] {
                HirExpr::MethodCall { args, span, .. } => (args.clone(), *span),
                _ => unreachable!("method call chain should contain only method calls"),
            };
            control = self.args_control(&args, control)?;
            control = self.apply_normal_step(control, |this, state| {
                this.semantics.method_call(call, span, state)
            })?;
            if call != expr {
                control = self.semantics.after_expr_control(call, control);
            }
        }
        Ok(control)
    }

    fn if_chain_control(
        &mut self,
        expr: HirExprId,
        state: S::Domain,
    ) -> Result<Control<S::Domain>, E> {
        let mut frames = Vec::new();
        let mut entry = state;
        let mut current = expr;

        let mut result = loop {
            (self.checkpoint)()?;
            let (cond, then_block, else_branch) =
                match self.context().program().exprs[current].clone() {
                    HirExpr::If {
                        cond,
                        then_block,
                        else_branch,
                        ..
                    } => (cond, then_block, else_branch),
                    _ => unreachable!("if chain should contain only if expressions"),
                };

            let mut cond_control = self.expr_control(cond, entry)?;
            let Some(base) = cond_control.take_normal_state() else {
                break if current == expr {
                    cond_control
                } else {
                    self.semantics.after_expr_control(current, cond_control)
                };
            };
            let then_control = self.block_control(then_block, base.clone())?;
            frames.push((current, cond_control, then_control));

            match else_branch {
                Some(HirElseBranch::If(next)) => {
                    (self.checkpoint)()?;
                    current = next;
                    entry = self.semantics.before_expr(next, base);
                }
                Some(HirElseBranch::Block(block)) => {
                    if let Some(next) = self.tail_if_expr_in_empty_block(block) {
                        (self.checkpoint)()?;
                        current = next;
                        entry = self.semantics.before_expr(next, base);
                        continue;
                    }
                    break self.block_control(block, base)?;
                }
                None => {
                    break Control::normal(base);
                }
            }
        };
        for (current, mut condition, then_control) in frames.into_iter().rev() {
            (self.checkpoint)()?;
            condition.join_assign(&then_control);
            condition.join_assign(&result);
            result = if current == expr {
                condition
            } else {
                self.semantics.after_expr_control(current, condition)
            };
        }
        Ok(result)
    }

    fn tail_if_expr_in_empty_block(&self, block: etas_hir::HirBlockId) -> Option<HirExprId> {
        let block = &self.context().program().blocks[block];
        if !block.stmts.is_empty() {
            return None;
        }
        let expr = block.final_expr?;
        matches!(
            self.context().program().exprs.get(expr),
            Some(HirExpr::If { .. })
        )
        .then_some(expr)
    }
}
