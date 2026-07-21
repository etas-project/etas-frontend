use etas_hir::{HirStmt, HirStmtId};

use crate::intraprocedural::{Control, HirAbstractInterpreter, HirAnalysisSemantics};

impl<S> HirAbstractInterpreter<S>
where
    S: HirAnalysisSemantics,
{
    pub fn stmt(&mut self, stmt: HirStmtId, state: S::Domain) -> S::Domain {
        self.stmt_control(stmt, state).into_joined_domain()
    }

    pub fn stmt_control(&mut self, stmt: HirStmtId, state: S::Domain) -> Control<S::Domain> {
        let state = self.semantics.before_stmt(stmt, state);
        let stmt_data = self.semantics.hir().stmts[stmt].clone();
        let next = match stmt_data {
            HirStmt::Let { value, .. } | HirStmt::Var { value, .. } => {
                self.expr_control(value, state)
            }
            HirStmt::Assign { target, value, .. } => {
                let control = self.expr_control(target, state);
                control.with_normal_processed(|state| self.expr_control(value, state))
            }
            HirStmt::If(expr) | HirStmt::Match(expr) | HirStmt::Expr { expr, .. } => {
                self.expr_control(expr, state)
            }
            HirStmt::For {
                iter, limits, body, ..
            } => {
                let control = self.expr_control(iter, state);
                let control = self.exprs_control(limits, control);
                control.with_normal_processed(|state| self.loop_block_control(body, state))
            }
            HirStmt::While {
                cond, limits, body, ..
            } => {
                let control = self.expr_control(cond, state);
                let control = self.exprs_control(limits, control);
                control.with_normal_processed(|state| self.loop_block_control(body, state))
            }
            HirStmt::Retry { limits, body, .. } => {
                let control = self.exprs_control(limits, Control::normal(state));
                control.with_normal_processed(|state| self.loop_block_control(body, state))
            }
            HirStmt::Return { value, .. } => value
                .map(|value| self.expr_control(value, state.clone()))
                .unwrap_or_else(|| Control::normal(state))
                .with_normal_processed(Control::returning),
            HirStmt::Resume { value, .. } => value
                .map(|value| self.expr_control(value, state.clone()))
                .unwrap_or_else(|| Control::normal(state))
                .with_normal_processed(Control::resuming),
            HirStmt::Finish { value, .. } => self
                .expr_control(value, state)
                .with_normal_processed(Control::finishing),
            HirStmt::Break { .. } => Control::breaking(state),
            HirStmt::Continue { .. } => Control::continuing(state),
            HirStmt::Error { .. } => Control::error(state),
        };
        self.map_control_states(next, |semantics, state| semantics.after_stmt(stmt, state))
    }
}
