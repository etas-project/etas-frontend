mod block;
mod call;
mod expr;
mod stmt;

#[cfg(test)]
mod tests;

use crate::HirAnalysisContext;
use crate::intraprocedural::{AnalysisStep, Control, HirAnalysisSemantics};

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
        control.map_states(|state| f(&mut self.semantics, state))
    }

    pub(crate) fn apply_normal_step(
        &mut self,
        control: Control<S::Domain>,
        mut f: impl FnMut(&mut Self, S::Domain) -> AnalysisStep<S::Domain>,
    ) -> Control<S::Domain> {
        control.with_normal_processed(|state| {
            let step = f(self, state);
            let state = self.require_handled(step);
            Control::normal(state)
        })
    }
}
