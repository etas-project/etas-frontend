mod candidate;
mod compiler;
mod definitions;
pub use definitions::NativeCompilerError;
mod report;
mod syntax;

pub use crate::code::pipeline::{CandidateAnalysisError, CandidateBodyAnalysis};
pub(crate) use candidate::CandidateEmission;
pub use candidate::{
    CandidatePublisher, CandidateResolutionError, CheckedCandidate, CheckedCandidateResolver,
};
pub use compiler::CandidateCompileError;
pub use report::CheckReportError;
pub(crate) use syntax::validate_retained_source;
pub use syntax::{NativeBodyLease, NativeCodeSyntaxOperations, NativeSyntaxLimits};
