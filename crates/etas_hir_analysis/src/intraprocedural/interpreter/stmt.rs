use etas_hir::{HirStmt, HirStmtId};

use super::Traversal;
use crate::intraprocedural::{Control, HirAnalysisSemantics};

impl<S, F, E> Traversal<'_, S, F>
where
    S: HirAnalysisSemantics,
    F: Fn() -> Result<(), E>,
{
    pub(super) fn stmt_control(
        &mut self,
        stmt: HirStmtId,
        state: S::Domain,
    ) -> Result<Control<S::Domain>, E> {
        (self.checkpoint)()?;
        let state = self.semantics.before_stmt(stmt, state);
        let stmt_data = self.context().program().stmts[stmt].clone();
        let mut bounded_for_complete = false;
        let next = match stmt_data {
            HirStmt::Let { value, .. } | HirStmt::Var { value, .. } => {
                self.expr_control(value, state)?
            }
            HirStmt::Assign { target, value, .. } => {
                let control = self.expr_control(target, state)?;
                control.try_with_normal_processed(|state| self.expr_control(value, state))?
            }
            HirStmt::If(expr) | HirStmt::Match(expr) | HirStmt::Expr { expr, .. } => {
                self.expr_control(expr, state)?
            }
            HirStmt::For {
                iter, limits, body, ..
            } => {
                let control = self.exprs_control(limits, Control::normal(state))?;
                let control =
                    control.try_with_normal_processed(|state| self.expr_control(iter, state))?;
                match self.semantics.bounded_for_iterations(stmt) {
                    Some(bound) => control.try_with_normal_processed(|state| {
                        let (result, complete) =
                            self.bounded_for_control(stmt, body, state, bound)?;
                        bounded_for_complete = complete;
                        Ok(result)
                    })?,
                    None => control
                        .try_with_normal_processed(|state| self.loop_block_control(body, state))?,
                }
            }
            HirStmt::While {
                cond, limits, body, ..
            } => {
                let control = self.exprs_control(limits, Control::normal(state))?;
                control
                    .try_with_normal_processed(|state| self.while_loop_control(cond, body, state))?
            }
            HirStmt::Retry { limits, body, .. } => {
                let control = self.exprs_control(limits, Control::normal(state))?;
                control.try_with_normal_processed(|state| self.loop_block_control(body, state))?
            }
            HirStmt::Return { value, .. } => {
                let control = match value {
                    Some(value) => self.expr_control(value, state)?,
                    None => Control::normal(state),
                };
                (self.checkpoint)()?;
                control.with_normal_processed(|state| {
                    Control::returning(self.semantics.return_value(stmt, value, state))
                })
            }
            HirStmt::Resume { value, .. } => {
                let control = match value {
                    Some(value) => self.expr_control(value, state)?,
                    None => Control::normal(state),
                };
                control.with_normal_processed(Control::resuming)
            }
            HirStmt::Finish { value, .. } => self
                .expr_control(value, state)?
                .with_normal_processed(Control::finishing),
            HirStmt::Break { .. } => Control::breaking(state),
            HirStmt::Continue { .. } => Control::continuing(state),
            HirStmt::Error { .. } => Control::error(state),
        };
        (self.checkpoint)()?;
        Ok(self.map_control_states(next, |semantics, state| {
            if bounded_for_complete {
                semantics.after_bounded_for(stmt, state)
            } else {
                semantics.after_stmt(stmt, state)
            }
        }))
    }
}
