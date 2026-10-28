pub(crate) mod baseline;
mod context;
pub(crate) mod passes;
mod run;

pub(crate) use context::CandidatePipelineContext;
pub(crate) use run::analyze_body;
pub(crate) use run::analyze_body_with_dependencies;
pub(crate) use run::analyze_controlled_body;
pub use run::{CandidateAnalysisError, CandidateBodyAnalysis};
