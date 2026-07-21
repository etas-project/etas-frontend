use etas_hir::HirBlockId;

use crate::intraprocedural::{Control, HirAbstractInterpreter, HirAnalysisSemantics};

impl<S> HirAbstractInterpreter<S>
where
    S: HirAnalysisSemantics,
{
    pub fn block(&mut self, block: HirBlockId, state: S::Domain) -> S::Domain {
        self.block_control(block, state).into_joined_domain()
    }

    pub fn block_control(&mut self, block: HirBlockId, state: S::Domain) -> Control<S::Domain> {
        let block = self.semantics.hir().blocks[block].clone();
        let mut control = Control::normal(state);
        for stmt in block.stmts {
            control = control.with_normal_processed(|state| self.stmt_control(stmt, state));
            if !control.has_normal() {
                break;
            }
        }
        if let Some(expr) = block.final_expr {
            control = control.with_normal_processed(|state| self.expr_control(expr, state));
        }
        control
    }
}
