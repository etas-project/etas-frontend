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
    pub(crate) fn bounded_for_control(
        &mut self,
        stmt: etas_hir::HirStmtId,
        body: HirBlockId,
        initial: S::Domain,
        max_iterations: usize,
    ) -> Result<(Control<S::Domain>, bool), E> {
        let mut result = Control::normal(initial.clone());
        let mut active = Some(initial);
        for _ in 0..max_iterations {
            let Some(state) = active.take() else { break };
            (self.checkpoint)()?;
            if self.loop_iterations_remaining == 0 {
                let remaining = self.loop_block_control(body, state)?;
                result.join_assign(&remaining);
                return Ok((result, false));
            }
            self.loop_iterations_remaining -= 1;
            let iteration = self.block_control(body, state)?;
            if let Some(state) = iteration.break_state() {
                result.join_normal(self.semantics.bounded_for_break(stmt, state.clone()));
            }
            if let Some(state) = iteration.return_state() {
                result.join_return(state.clone());
            }
            if let Some(state) = iteration.resume_state() {
                result.join_resume(state.clone());
            }
            if let Some(state) = iteration.finish_state() {
                result.join_finish(state.clone());
            }
            if let Some(state) = iteration.error_state() {
                result.join_error(state.clone());
            }
            let mut next = Control::bottom();
            if let Some(state) = iteration.normal_state() {
                next.join_normal(state.clone());
            }
            if let Some(state) = iteration.continue_state() {
                next.join_normal(state.clone());
            }
            active = next.take_normal_state();
            if let Some(state) = &active {
                result.join_normal(state.clone());
            }
        }
        Ok((result, true))
    }

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
                self.semantics.loop_fixpoint_incomplete(body, solved.value)
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

    pub(crate) fn while_loop_control(
        &mut self,
        cond: HirExprId,
        body: HirBlockId,
        initial: S::Domain,
    ) -> Result<Control<S::Domain>, E> {
        let mut result = self.expr_control(cond, initial.clone())?;
        let solved = FixpointEngine::default().try_solve(initial, |state| {
            let condition = self.expr_control(cond, state.clone())?;
            let Some(condition_state) = condition.normal_state() else {
                return Ok(false);
            };
            let body_control = self.block_control(body, condition_state.clone())?;
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
                self.semantics.loop_fixpoint_incomplete(body, solved.value)
            }
        };

        result.join_assign(&self.expr_control(cond, fixed)?);
        if let Some(condition_state) = result.normal_state().cloned() {
            let body_control = self.block_control(body, condition_state)?;
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
