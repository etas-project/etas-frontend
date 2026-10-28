use etas_code::{descriptor::DescriptorLimits, report::CheckReport};

use super::{CandidateAnalysisError, CandidateBodyAnalysis, CheckReportError, CheckedCandidate};

#[derive(Debug)]
pub enum CandidateCompileError {
    Analysis(CandidateAnalysisError),
    Report(CheckReportError),
    Rejected(Box<CheckReport>),
}

impl std::fmt::Display for CandidateCompileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(report) => write!(
                formatter,
                "candidate rejected with {} diagnostics",
                report.diagnostics.len()
            ),
            Self::Analysis(error) => {
                write!(formatter, "candidate compiler pipeline failed: {error:?}")
            }
            Self::Report(error) => error.fmt(formatter),
        }
    }
}
impl std::error::Error for CandidateCompileError {}

impl CandidateBodyAnalysis {
    pub fn into_checked_candidate(
        mut self,
        limits: DescriptorLimits,
    ) -> Result<CheckedCandidate, CandidateCompileError> {
        self.definition_catalog(limits)
            .map_err(CheckReportError::Projection)
            .map_err(CandidateCompileError::Report)?;
        if self
            .validate_emission()
            .map_err(CandidateCompileError::Report)?
        {
            return self.take_emitted().ok_or({
                CandidateCompileError::Report(CheckReportError::IncompleteAnalysis(
                    "validated candidate emission disappeared",
                ))
            });
        }
        let report = self
            .check_report(limits)
            .map_err(CandidateCompileError::Report)?;
        Err(CandidateCompileError::Rejected(Box::new(report)))
    }
}
