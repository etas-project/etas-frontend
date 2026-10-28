use etas_utils::FixpointEngine;

use crate::intraprocedural::HirAnalysisSemantics;

use super::{
    AnalysisUnit, CallGraph, ComponentConvergence, InterproceduralDiagnostic,
    InterproceduralSemantics, SummarySolver, SummaryStore,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterproceduralAnalysisResult<U, Summary, Semantics>
where
    U: AnalysisUnit,
{
    pub semantics: Semantics,
    pub summaries: SummaryStore<U, Summary>,
    pub call_graph: CallGraph<U>,
    pub convergence: Vec<ComponentConvergence<U>>,
    pub diagnostics: Vec<InterproceduralDiagnostic<U>>,
}

#[derive(Clone, Debug)]
pub struct InterproceduralAnalysis<S>
where
    S: InterproceduralSemantics,
{
    units: Vec<S::Unit>,
    semantics: S,
    engine: FixpointEngine,
}

impl<S> InterproceduralAnalysis<S>
where
    S: InterproceduralSemantics
        + HirAnalysisSemantics<Domain = <S as InterproceduralSemantics>::Domain>,
{
    pub fn new(units: impl IntoIterator<Item = S::Unit>, semantics: S) -> Self {
        Self {
            units: units.into_iter().collect(),
            semantics,
            engine: FixpointEngine::default(),
        }
    }

    pub fn with_engine(mut self, engine: FixpointEngine) -> Self {
        self.engine = engine;
        self
    }

    pub fn solve(self) -> InterproceduralAnalysisResult<S::Unit, S::Summary, S> {
        self.solve_with_checkpoint(&|| Ok::<(), std::convert::Infallible>(()))
            .unwrap_or_else(|never| match never {})
    }

    pub fn solve_controlled(
        self,
        control: &dyn etas_utils::PipelineExecutionControl,
    ) -> Result<
        InterproceduralAnalysisResult<S::Unit, S::Summary, S>,
        etas_utils::PipelineInterruption,
    > {
        self.solve_with_checkpoint(&|| control.checkpoint())
    }

    fn solve_with_checkpoint<E>(
        mut self,
        checkpoint: &impl Fn() -> Result<(), E>,
    ) -> Result<InterproceduralAnalysisResult<S::Unit, S::Summary, S>, E> {
        let solver = SummarySolver::new(self.engine);
        let result = solver.solve_with_checkpoint(self.units, &mut self.semantics, checkpoint)?;
        Ok(InterproceduralAnalysisResult {
            semantics: self.semantics,
            summaries: result.summaries,
            call_graph: result.call_graph,
            convergence: result.convergence,
            diagnostics: result.diagnostics,
        })
    }
}
