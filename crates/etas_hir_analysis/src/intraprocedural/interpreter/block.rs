use etas_hir::HirBlockId;

use super::Traversal;
use crate::intraprocedural::{Control, HirAnalysisSemantics};

impl<S, F, E> Traversal<'_, S, F>
where
    S: HirAnalysisSemantics,
    F: Fn() -> Result<(), E>,
{
    pub(super) fn block_control(
        &mut self,
        block: HirBlockId,
        state: S::Domain,
    ) -> Result<Control<S::Domain>, E> {
        (self.checkpoint)()?;
        let block = self.context().program().blocks[block].clone();
        let mut control = Control::normal(state);
        for stmt in block.stmts {
            control = control.try_with_normal_processed(|state| self.stmt_control(stmt, state))?;
            if !control.has_normal() {
                break;
            }
        }
        if let Some(expr) = block.final_expr {
            control = control.try_with_normal_processed(|state| self.expr_control(expr, state))?;
        }
        Ok(control)
    }
}
