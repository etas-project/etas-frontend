mod adapter;
mod analysis;
mod body;
mod call_graph;
mod call_site;
mod context;
mod diagnostic;
mod semantics;
mod solver;
mod store;
mod target;
mod unit;

#[cfg(test)]
mod tests;

pub use analysis::{InterproceduralAnalysis, InterproceduralAnalysisResult};
pub use body::HirAnalysisBody;
pub use call_graph::CallGraph;
pub use call_site::{CallSite, ResolvedCallee};
pub use context::{AnalysisPhase, UnitContext};
pub use diagnostic::InterproceduralDiagnostic;
pub use semantics::InterproceduralSemantics;
pub use solver::{ComponentConvergence, SummarySolver, SummarySolverResult};
pub use store::SummaryStore;
pub use target::CallTarget;
pub use unit::AnalysisUnit;
