use etas_hir::{HirArg, HirElseBranch, HirExpr, HirExprId, HirFieldInit, HirMatchArmBody};

use crate::intraprocedural::{
    Control, HandleParts, HandlerParts, HirAbstractInterpreter, HirAnalysisSemantics,
};

impl<S> HirAbstractInterpreter<S>
where
    S: HirAnalysisSemantics,
{
    pub fn expr(&mut self, expr: HirExprId, state: S::Domain) -> S::Domain {
        self.expr_control(expr, state).into_joined_domain()
    }

    pub fn expr_control(&mut self, expr: HirExprId, state: S::Domain) -> Control<S::Domain> {
        let state = self.semantics.before_expr(expr, state);
        let expr_data = self.semantics.hir().exprs[expr].clone();
        let span = expr_data.span(&self.semantics.hir().blocks);
        let next = match expr_data {
            HirExpr::Perform {
                action,
                generic_args,
                args,
                ..
            } => {
                let control = self.args_control(&args, Control::normal(state));
                self.apply_normal_step(control, |this, state| {
                    this.semantics
                        .perform(expr, &action, &generic_args, &args, span, state)
                })
            }
            HirExpr::If { .. } => self.if_chain_control(expr, state),
            HirExpr::Match {
                scrutinee, arms, ..
            } => {
                let control = self.expr_control(scrutinee, state);
                control.with_normal_processed(|base| {
                    let mut joined = Control::bottom();
                    for arm in arms {
                        let arm_state = match arm.body {
                            HirMatchArmBody::Expr(expr) => self.expr_control(expr, base.clone()),
                            HirMatchArmBody::Block(block) => {
                                self.block_control(block, base.clone())
                            }
                        };
                        joined.join_assign(&arm_state);
                    }
                    joined
                })
            }
            HirExpr::Block(block) => self.block_control(block, state),
            HirExpr::Call { callee, args, .. } => {
                let control = self.expr_control(callee, state);
                let control = self.args_control(&args, control);
                control.with_normal_processed(|state| {
                    self.direct_call_control(expr, callee, span, state)
                })
            }
            HirExpr::MethodCall { .. } => self.method_call_chain_control(expr, state),
            HirExpr::SpecMethodCall { receiver, args, .. } => {
                let control = self.expr_control(receiver, state);
                self.args_control(&args, control)
            }
            HirExpr::Record(record) => {
                self.record_fields_control(record.fields, Control::normal(state))
            }
            HirExpr::Tuple { elems, .. }
            | HirExpr::Array { elems, .. }
            | HirExpr::List { elems, .. }
            | HirExpr::Set { elems, .. } => self.exprs_control(elems, Control::normal(state)),
            HirExpr::Map { entries, .. } => {
                entries
                    .into_iter()
                    .fold(Control::normal(state), |control, entry| {
                        let control = control
                            .with_normal_processed(|state| self.expr_control(entry.key, state));
                        control.with_normal_processed(|state| self.expr_control(entry.value, state))
                    })
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
                let control = self.expr_control(head, state);
                control.with_normal_processed(|state| self.expr_control(tail, state))
            }
            HirExpr::Field { base, .. } | HirExpr::Unary { expr: base, .. } => {
                self.expr_control(base, state)
            }
            HirExpr::Try { expr: base, .. } => {
                let control = self.expr_control(base, state);
                self.apply_normal_step(control, |this, state| {
                    this.semantics.try_expr(expr, span, state)
                })
            }
            HirExpr::Index { base, index, .. } => {
                let control = self.expr_control(base, state);
                control.with_normal_processed(|state| self.expr_control(index, state))
            }
            HirExpr::Slice {
                base, start, end, ..
            } => {
                let control = self.expr_control(base, state);
                let control =
                    control.with_normal_processed(|state| self.expr_control(start, state));
                control.with_normal_processed(|state| self.expr_control(end, state))
            }
            HirExpr::Handle { body, handler, .. } => {
                let entry = state.clone();
                let body_control = self.expr_control(body, state);
                let handler_parts = HandlerParts::Expr {
                    expr: handler,
                    control: self.expr_control(handler, entry.clone()),
                };
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
                let control = stages
                    .into_iter()
                    .fold(Control::normal(state), |control, stage| {
                        stage.limits.into_iter().fold(
                            control.with_normal_processed(|state| {
                                self.expr_control(stage.expr, state)
                            }),
                            |control, limit| {
                                control
                                    .with_normal_processed(|state| self.expr_control(limit, state))
                            },
                        )
                    });
                self.apply_normal_step(control, |this, state| {
                    this.semantics.stage_compose(expr, span, state)
                })
            }
            HirExpr::Pipeline { input, stages, .. } => {
                let control = self.expr_control(input, state);
                let control = stages.into_iter().fold(control, |control, stage| {
                    stage.limits.into_iter().fold(
                        control.with_normal_processed(|state| self.expr_control(stage.expr, state)),
                        |control, limit| {
                            control.with_normal_processed(|state| self.expr_control(limit, state))
                        },
                    )
                });
                self.apply_normal_step(control, |this, state| {
                    this.semantics.pipeline(expr, span, state)
                })
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
            | HirExpr::EmptyRecordOrMap { .. }
            | HirExpr::EmptySequence { .. }
            | HirExpr::Error { .. } => Control::normal(state),
        };
        self.map_control_states(next, |semantics, state| semantics.after_expr(expr, state))
    }

    pub(crate) fn args_control(
        &mut self,
        args: &[HirArg],
        control: Control<S::Domain>,
    ) -> Control<S::Domain> {
        args.iter().fold(control, |control, arg| match arg {
            HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => {
                control.with_normal_processed(|state| self.expr_control(*expr, state))
            }
        })
    }

    pub(crate) fn exprs_control(
        &mut self,
        exprs: impl IntoIterator<Item = HirExprId>,
        control: Control<S::Domain>,
    ) -> Control<S::Domain> {
        exprs.into_iter().fold(control, |control, expr| {
            control.with_normal_processed(|state| self.expr_control(expr, state))
        })
    }

    fn record_fields_control(
        &mut self,
        fields: impl IntoIterator<Item = HirFieldInit>,
        control: Control<S::Domain>,
    ) -> Control<S::Domain> {
        fields
            .into_iter()
            .fold(control, |control, field| match field {
                HirFieldInit::Named { value, .. } => {
                    control.with_normal_processed(|state| self.expr_control(value, state))
                }
                HirFieldInit::Shorthand { .. } => control,
            })
    }

    fn method_call_chain_control(
        &mut self,
        expr: HirExprId,
        state: S::Domain,
    ) -> Control<S::Domain> {
        let mut chain = Vec::new();
        let mut current = expr;
        while let HirExpr::MethodCall { receiver, .. } = &self.semantics.hir().exprs[current] {
            chain.push(current);
            current = *receiver;
        }

        let mut control = self.expr_control(current, state);
        for call in chain.into_iter().rev() {
            let (args, span) = match &self.semantics.hir().exprs[call] {
                HirExpr::MethodCall { args, span, .. } => (args.clone(), *span),
                _ => unreachable!("method call chain should contain only method calls"),
            };
            control = self.args_control(&args, control);
            control = self.apply_normal_step(control, |this, state| {
                this.semantics.method_call(call, span, state)
            });
        }
        control
    }

    fn if_chain_control(&mut self, expr: HirExprId, state: S::Domain) -> Control<S::Domain> {
        let mut result = Control::bottom();
        let mut else_control = Control::normal(state);
        let mut current = expr;

        loop {
            let (cond, then_block, else_branch) = match self.semantics.hir().exprs[current].clone()
            {
                HirExpr::If {
                    cond,
                    then_block,
                    else_branch,
                    ..
                } => (cond, then_block, else_branch),
                _ => unreachable!("if chain should contain only if expressions"),
            };

            let mut cond_control =
                else_control.with_normal_processed(|state| self.expr_control(cond, state));
            let Some(base) = cond_control.take_normal_state() else {
                result.join_assign(&cond_control);
                return result;
            };
            result.join_assign(&cond_control);
            result.join_assign(&self.block_control(then_block, base.clone()));

            match else_branch {
                Some(HirElseBranch::If(next)) => {
                    current = next;
                    else_control = Control::normal(base);
                }
                Some(HirElseBranch::Block(block)) => {
                    if let Some(next) = self.tail_if_expr_in_empty_block(block) {
                        current = next;
                        else_control = Control::normal(base);
                        continue;
                    }
                    result.join_assign(&self.block_control(block, base));
                    return result;
                }
                None => {
                    result.join_normal(base);
                    return result;
                }
            }
        }
    }

    fn tail_if_expr_in_empty_block(&self, block: etas_hir::HirBlockId) -> Option<HirExprId> {
        let block = &self.semantics.hir().blocks[block];
        if !block.stmts.is_empty() {
            return None;
        }
        let expr = block.final_expr?;
        matches!(
            self.semantics.hir().exprs.get(expr),
            Some(HirExpr::If { .. })
        )
        .then_some(expr)
    }
}
