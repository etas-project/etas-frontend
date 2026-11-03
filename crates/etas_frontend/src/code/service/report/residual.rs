use std::collections::BTreeSet;

use crate::code::{
    contract::{CandidateContractAnalysis, ContractRelation},
    service::{CandidateBodyAnalysis, CheckReportError},
};
use etas_code::{
    diagnostic::DiagnosticLocation, report::RuntimeCheckInfo, residual::model::ResidualKind,
};

/// A descriptive projection of exact supported obligations. It neither
/// installs native guards nor creates permission from a report string.
pub(super) fn runtime_checks(
    analysis: &CandidateBodyAnalysis,
) -> Result<Vec<RuntimeCheckInfo>, CheckReportError> {
    let Some(CandidateContractAnalysis::Analyzed { checks, .. }) = analysis.contract_analysis()
    else {
        return Ok(Vec::new());
    };
    let required = checks
        .iter()
        .filter_map(|check| match &check.relation {
            ContractRelation::Guarded(required) => Some(required.iter()),
            _ => None,
        })
        .flatten()
        .collect::<BTreeSet<_>>();
    if required.is_empty() {
        return Ok(Vec::new());
    }
    let checked =
        analysis
            .output()
            .checked
            .as_ref()
            .ok_or(CheckReportError::IncompleteAnalysis(
                "guarded report has no checked program",
            ))?;
    required
        .into_iter()
        .map(|reference| {
            let mut plans = checked
                .residual_plans()
                .plans()
                .filter(|plan| plan.references().contains(reference));
            let plan = plans.next().ok_or(CheckReportError::IncompleteAnalysis(
                "guarded report has no verified obligation plan",
            ))?;
            if plans.next().is_some() {
                return Err(CheckReportError::IncompleteAnalysis(
                    "guarded report has duplicate obligation coverage",
                ));
            }
            let obligation = plan
                .plan()
                .obligations()
                .get(reference.obligation())
                .ok_or(CheckReportError::IncompleteAnalysis(
                    "guarded report has no semantic obligation",
                ))?;
            let origin = &obligation.identity().origin;
            let kind = match obligation.kind() {
                ResidualKind::ActionPattern { .. } => "action_predicate",
                ResidualKind::TraceContract { .. } => "trace_contract",
                ResidualKind::ExistingGuard { .. } => "existing_guard",
            };
            Ok(RuntimeCheckInfo {
                kind: kind.into(),
                location: Some(DiagnosticLocation {
                    source: origin.source.fingerprint(),
                    revision: origin.revision.fingerprint(),
                    byte_start: usize::try_from(origin.byte_start).map_err(|_| {
                        CheckReportError::IncompleteAnalysis(
                            "residual source range exceeds the report ABI",
                        )
                    })?,
                    byte_end: usize::try_from(origin.byte_end).map_err(|_| {
                        CheckReportError::IncompleteAnalysis(
                            "residual source range exceeds the report ABI",
                        )
                    })?,
                }),
                spec: None,
            })
        })
        .collect()
}
