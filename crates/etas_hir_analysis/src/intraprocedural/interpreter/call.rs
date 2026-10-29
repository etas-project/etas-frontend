use etas_core::Span;
use etas_hir::{HirBlockId, HirExprId};
use etas_utils::{ConvergenceStatus, FixpointEngine, JoinSemiLattice};

use super::Traversal;
use crate::intraprocedural::{Control, HirAnalysisSemantics};

impl<S, F, E> Traversal<'_, S, F>
where
    S: HirAnalysisSemantics,
    F: Fn() -> Result<(), E>,
{
    pub(crate) fn loop_block_control(
        &mut self,
        body: HirBlockId,
        initial: S::Domain,
    ) -> Result<Control<S::Domain>, E> {
        let solved = FixpointEngine::default().try_solve(initial, |state| {
            let body_control = self.block_control(body, state.clone())?;
            let mut back_edge = S::Domain::bottom();
            if let Some(normal) = body_control.normal_state() {
                back_edge.join_assign(normal);
            }
            if let Some(continued) = body_control.continue_state() {
                back_edge.join_assign(continued);
            }
            Ok(state.join_assign(&back_edge))
        })?;
        let fixed = match solved.status {
            ConvergenceStatus::Converged => solved.value,
            ConvergenceStatus::IterationLimitReached => {
                self.semantics.incomplete_facts(solved.value)
            }
        };

        let body_control = self.block_control(body, fixed.clone())?;
        let mut result = Control::normal(fixed);
        if let Some(broken) = body_control.break_state() {
            result.join_normal(broken.clone());
        }
        if let Some(returned) = body_control.return_state() {
            result.join_return(returned.clone());
        }
        if let Some(resumed) = body_control.resume_state() {
            result.join_resume(resumed.clone());
        }
        if let Some(finished) = body_control.finish_state() {
            result.join_finish(finished.clone());
        }
        if let Some(errored) = body_control.error_state() {
            result.join_error(errored.clone());
        }
        Ok(result)
    }

    pub(crate) fn direct_call_control(
        &mut self,
        call: HirExprId,
        callee: HirExprId,
        span: Span,
        state: S::Domain,
    ) -> Result<Control<S::Domain>, E> {
        (self.checkpoint)()?;
        let step = self.semantics.direct_call(call, callee, span, state);
        Ok(Control::normal(self.require_handled(step)))
    }
}
