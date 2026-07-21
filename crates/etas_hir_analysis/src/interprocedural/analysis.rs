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

    pub fn solve(mut self) -> InterproceduralAnalysisResult<S::Unit, S::Summary, S> {
        let solver = SummarySolver::new(self.engine);
        let result = solver.solve(self.units, &mut self.semantics);
        InterproceduralAnalysisResult {
            semantics: self.semantics,
            summaries: result.summaries,
            call_graph: result.call_graph,
            convergence: result.convergence,
            diagnostics: result.diagnostics,
        }
    }
}
