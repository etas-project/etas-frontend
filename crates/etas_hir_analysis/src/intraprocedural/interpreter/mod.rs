mod block;
mod call;
mod expr;
mod stmt;
mod structural;

#[cfg(test)]
mod tests;

use crate::HirAnalysisContext;
use crate::intraprocedural::{AnalysisStep, Control, HirAnalysisSemantics};
use etas_hir::{HirBlockId, HirExprId, HirStmtId};

pub struct HirAbstractInterpreter<S>
where
    S: HirAnalysisSemantics,
{
    semantics: S,
}

impl<S> HirAbstractInterpreter<S>
where
    S: HirAnalysisSemantics,
{
    pub fn new(semantics: S) -> Self {
        Self { semantics }
    }

    pub fn semantics(&self) -> &S {
        &self.semantics
    }

    pub fn context(&self) -> &HirAnalysisContext {
        self.semantics.analysis_context()
    }

    pub fn semantics_mut(&mut self) -> &mut S {
        &mut self.semantics
    }

    pub fn into_semantics(self) -> S {
        self.semantics
    }

    pub fn block(&mut self, block: HirBlockId, state: S::Domain) -> S::Domain {
        self.block_control(block, state).into_joined_domain()
    }

    pub fn block_control(&mut self, block: HirBlockId, state: S::Domain) -> Control<S::Domain> {
        self.try_block_control(block, state, &|| Ok::<(), std::convert::Infallible>(()))
            .unwrap_or_else(|never| match never {})
    }

    pub fn try_block_control<E>(
        &mut self,
        block: HirBlockId,
        state: S::Domain,
        checkpoint: &impl Fn() -> Result<(), E>,
    ) -> Result<Control<S::Domain>, E> {
        Traversal {
            semantics: &mut self.semantics,
            checkpoint,
        }
        .block_control(block, state)
    }

    pub fn stmt(&mut self, stmt: HirStmtId, state: S::Domain) -> S::Domain {
        self.stmt_control(stmt, state).into_joined_domain()
    }

    pub fn stmt_control(&mut self, stmt: HirStmtId, state: S::Domain) -> Control<S::Domain> {
        self.try_stmt_control(stmt, state, &|| Ok::<(), std::convert::Infallible>(()))
            .unwrap_or_else(|never| match never {})
    }

    pub fn try_stmt_control<E>(
        &mut self,
        stmt: HirStmtId,
        state: S::Domain,
        checkpoint: &impl Fn() -> Result<(), E>,
    ) -> Result<Control<S::Domain>, E> {
        Traversal {
            semantics: &mut self.semantics,
            checkpoint,
        }
        .stmt_control(stmt, state)
    }

    pub fn expr(&mut self, expr: HirExprId, state: S::Domain) -> S::Domain {
        self.expr_control(expr, state).into_joined_domain()
    }

    pub fn expr_control(&mut self, expr: HirExprId, state: S::Domain) -> Control<S::Domain> {
        self.try_expr_control(expr, state, &|| Ok::<(), std::convert::Infallible>(()))
            .unwrap_or_else(|never| match never {})
    }

    pub fn try_expr_control<E>(
        &mut self,
        expr: HirExprId,
        state: S::Domain,
        checkpoint: &impl Fn() -> Result<(), E>,
    ) -> Result<Control<S::Domain>, E> {
        Traversal {
            semantics: &mut self.semantics,
            checkpoint,
        }
        .expr_control(expr, state)
    }
}

struct Traversal<'a, S, F> {
    semantics: &'a mut S,
    checkpoint: &'a F,
}

impl<S, F, E> Traversal<'_, S, F>
where
    S: HirAnalysisSemantics,
    F: Fn() -> Result<(), E>,
{
    fn context(&self) -> &HirAnalysisContext {
        self.semantics.analysis_context()
    }

    pub(crate) fn require_handled(&mut self, step: AnalysisStep<S::Domain>) -> S::Domain {
        match step {
            AnalysisStep::Handled(state) => state,
            AnalysisStep::Unhandled { state, reason } => {
                self.semantics.unhandled_semantics(reason, state)
            }
        }
    }

    pub(crate) fn map_control_states(
        &mut self,
        control: Control<S::Domain>,
        mut f: impl FnMut(&mut S, S::Domain) -> S::Domain,
    ) -> Control<S::Domain> {
        control.map_states(|state| f(self.semantics, state))
    }

    pub(crate) fn apply_normal_step(
        &mut self,
        control: Control<S::Domain>,
        mut f: impl FnMut(&mut Self, S::Domain) -> AnalysisStep<S::Domain>,
    ) -> Result<Control<S::Domain>, E> {
        (self.checkpoint)()?;
        Ok(control.with_normal_processed(|state| {
            let step = f(self, state);
            let state = self.require_handled(step);
            Control::normal(state)
        }))
    }
}
