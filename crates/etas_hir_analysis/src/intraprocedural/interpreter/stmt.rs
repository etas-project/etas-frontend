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
                let control = self.expr_control(iter, state)?;
                let control = self.exprs_control(limits, control)?;
                control.try_with_normal_processed(|state| self.loop_block_control(body, state))?
            }
            HirStmt::While {
                cond, limits, body, ..
            } => {
                let control = self.expr_control(cond, state)?;
                let control = self.exprs_control(limits, control)?;
                control.try_with_normal_processed(|state| self.loop_block_control(body, state))?
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
        Ok(self.map_control_states(next, |semantics, state| semantics.after_stmt(stmt, state)))
    }
}
