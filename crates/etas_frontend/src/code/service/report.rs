use super::CandidateBodyAnalysis;
use crate::code::{
    contract::CandidateContractAnalysis,
    descriptor::ProjectionError,
    diagnostic::{DiagnosticProjector, contract_diagnostic},
};
use etas_code::{
    descriptor::{CallableAnalysisInfo, DescriptorLimits, EffectSummaryInfo},
    diagnostic::DiagnosticSeverity,
    report::{CheckReport, CheckStatus},
};

#[derive(Debug)]
pub enum CheckReportError {
    Projection(ProjectionError),
    MissingSource(etas_core::SourceId),
    InvalidSpan(etas_core::Span),
    IncompleteAnalysis(&'static str),
    InvalidContract(String),
}

impl From<ProjectionError> for CheckReportError {
    fn from(error: ProjectionError) -> Self {
        Self::Projection(error)
    }
}

impl std::fmt::Display for CheckReportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cannot project checked code report: {self:?}")
    }
}
impl std::error::Error for CheckReportError {}

impl CandidateBodyAnalysis {
    /// Projects actual compiler facts. This never publishes an implementation
    /// and cannot turn a user-created report into executable evidence.
    pub fn check_report(&self, limits: DescriptorLimits) -> Result<CheckReport, CheckReportError> {
        let mut locations = DiagnosticProjector::new(self)?;
        let mut diagnostics = self
            .output()
            .diagnostics
            .iter()
            .map(|diagnostic| locations.project(diagnostic))
            .collect::<Result<Vec<_>, _>>()?;
        let source_rejected = diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error);
        let catalog = self.definition_catalog(limits)?;
        let frozen = self.frozen().contract();
        let default = frozen.entry().ok_or(CheckReportError::IncompleteAnalysis(
            "candidate has no frozen default entry",
        ))?;
        let expected = frozen
            .callables()
            .find(|(id, _)| *id == default)
            .map(|(_, header)| header)
            .ok_or(CheckReportError::IncompleteAnalysis(
                "frozen default signature is missing",
            ))?;
        let (descriptors, analysis) = match &catalog {
            Some(catalog) => {
                let callable =
                    catalog
                        .callable(default)
                        .ok_or(CheckReportError::IncompleteAnalysis(
                            "candidate default signature is missing",
                        ))?;
                (catalog.descriptors().clone(), callable.analysis.clone())
            }
            None => (
                frozen.descriptors().clone(),
                CallableAnalysisInfo {
                    effects: EffectSummaryInfo {
                        declared_bound: expected.analysis.effects.declared_bound,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ),
        };
        let compatible = match (self.contract_analysis(), &catalog) {
            (
                Some(analysis @ CandidateContractAnalysis::Analyzed { checks, .. }),
                Some(catalog),
            ) => {
                let proven = analysis
                    .validate_for(self.fingerprint(), frozen)
                    .map_err(|error| CheckReportError::InvalidContract(error.to_string()))?;
                for check in checks {
                    if let Some(diagnostic) =
                        contract_diagnostic(self, catalog, check, &mut locations)?
                    {
                        diagnostics.push(diagnostic);
                    }
                }
                proven
            }
            (Some(CandidateContractAnalysis::SourceRejected { snapshot }), None)
                if source_rejected && *snapshot == self.fingerprint() =>
            {
                false
            }
            (None, None) if source_rejected => false,
            _ => {
                return Err(CheckReportError::IncompleteAnalysis(
                    "checked source and contract result disagree",
                ));
            }
        };
        let emitted = self.validate_emission()?;
        if compatible && !emitted {
            return Err(CheckReportError::IncompleteAnalysis(
                "proven candidate has no native emission artifact",
            ));
        }
        Ok(CheckReport {
            status: if compatible && !source_rejected {
                CheckStatus::Accepted
            } else {
                CheckStatus::Rejected
            },
            diagnostics,
            revision: self.revision().clone(),
            dependencies: self.frozen().dependency_info()?,
            snapshot_fingerprint: self.fingerprint(),
            descriptors,
            effects: analysis.effects,
            determinism: analysis.determinism,
            trace: analysis.trace,
            // The current native backend cannot install residual guards. Its
            // unsupported obligations above are rejection diagnostics.
            runtime_checks: Vec::new(),
        })
    }
}
