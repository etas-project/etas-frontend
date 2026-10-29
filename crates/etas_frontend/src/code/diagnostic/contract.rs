use std::sync::Arc;

use crate::code::{
    contract::{
        CallableContractCheck, ContractComponent as C, ContractRejection as R, ContractRelation,
        ContractUncertainty as U,
    },
    definition::DefinitionCatalog,
    service::{CandidateBodyAnalysis, CheckReportError},
};
use etas_code::{
    descriptor::{CallableHeaderInfo, DescriptorGraph},
    diagnostic::{
        AnalysisStep, DescriptorDetail, DiagnosticDetails, DiagnosticRecord, DiagnosticSeverity,
        ViolatedConstraint,
    },
};

pub(crate) fn contract_diagnostic(
    analysis: &CandidateBodyAnalysis,
    catalog: &DefinitionCatalog,
    check: &CallableContractCheck,
    locations: &mut super::DiagnosticProjector<'_>,
) -> Result<Option<DiagnosticRecord>, CheckReportError> {
    let (code, message, witness) = match &check.relation {
        ContractRelation::Proven => return Ok(None),
        ContractRelation::Rejected(rejection) => match rejection {
            R::ChangedInterface => (
                "code::ChangedInterface",
                "candidate changes the frozen callable interface",
                None,
            ),
            R::AdditionalEffects => match check.component {
                C::RequestedActions => (
                    "code::AdditionalRequestedActions",
                    "candidate requests actions outside the frozen caller-visible bound",
                    None,
                ),
                _ => (
                    "code::AdditionalEscapingEffects",
                    "candidate escapes effects outside the frozen interface bound",
                    None,
                ),
            },
            R::WeakenedDeterminism => (
                "code::WeakenedDeterminism",
                "candidate weakens the frozen determinism guarantee",
                None,
            ),
            R::ChangedRequestInstantiation => (
                "code::ChangedRequestInstantiation",
                "candidate changes the payload type instances visible to caller-installed handlers",
                None,
            ),
            R::ActionTraceNotContained(witness) => (
                "code::ActionTraceNotContained",
                "candidate action traces are not contained in the frozen contract",
                Some(witness.clone()),
            ),
            R::OutcomeTraceNotContained(witness) => (
                "code::OutcomeTraceNotContained",
                "candidate outcome-sensitive traces are not contained in the frozen contract",
                Some(witness.clone()),
            ),
        },
        ContractRelation::Unproven(reason) => match reason {
            U::MissingFrozenAnalysis => (
                "code::MissingFrozenAnalysis",
                "frozen behavior analysis is unavailable; compatibility is not proved",
                None,
            ),
            U::MissingCandidateAnalysis => (
                "code::MissingCandidateAnalysis",
                "candidate behavior analysis is unavailable; compatibility is not proved",
                None,
            ),
            U::MissingOutcomeSensitiveSummary => (
                "code::MissingOutcomeSensitiveSummary",
                "outcome-sensitive behavior is unavailable; compatibility is not proved",
                None,
            ),
            U::MissingLatentBehaviorSummary => (
                "code::MissingLatentBehaviorSummary",
                "returned callable behavior is unavailable; compatibility is not proved",
                None,
            ),
            U::UnprovenParametricDeterminism => (
                "code::UnprovenParametricDeterminism",
                "callback-dependent determinism cannot be proved from the checked trace",
                None,
            ),
            U::UnsupportedResidualChecks => (
                "code::UnsupportedResidualChecks",
                "required residual checks cannot be installed by this compilation backend",
                None,
            ),
            U::TraceProofLimit { .. } => (
                "code::TraceProofLimit",
                "trace proof resource limit was reached; compatibility is not proved",
                None,
            ),
        },
    };
    let frozen = analysis.frozen().contract();
    let expected = frozen
        .callables()
        .find(|(id, _)| *id == &check.callable)
        .map(|(_, header)| header)
        .ok_or(CheckReportError::IncompleteAnalysis(
            "contract check has no frozen callable",
        ))?;
    let actual = catalog.callable(&check.callable);
    let location = if frozen.entry() == Some(&check.callable) {
        let (source, _) = analysis.candidate_source();
        let source = analysis
            .output()
            .sources
            .as_ref()
            .and_then(|sources| sources.files.iter().find(|file| file.id == source))
            .ok_or(CheckReportError::MissingSource(source))?;
        Some(locations.location(etas_core::Span::new(
            source.id,
            etas_core::TextRange::new(
                etas_core::TextSize::ZERO,
                etas_core::TextSize::new(source.text.len()),
            ),
        ))?)
    } else {
        catalog
            .item_owner(&check.callable)
            .and_then(|item| {
                analysis
                    .output()
                    .checked
                    .as_ref()?
                    .items
                    .get(item)
                    .map(|item| item.span())
            })
            .map(|span| locations.location(span))
            .transpose()?
    };
    Ok(Some(DiagnosticRecord {
        code: code.into(),
        severity: DiagnosticSeverity::Error,
        message: message.into(),
        location: location.clone(),
        details: Some(Arc::new(DiagnosticDetails {
            expected: descriptor(check.component, frozen.descriptors(), expected, true),
            actual: actual.and_then(|header| {
                descriptor(check.component, catalog.descriptors(), header, false)
            }),
            constraint: Some(ViolatedConstraint {
                kind: component_name(check.component).into(),
                owner: Some(analysis.frozen().identity().clone()),
                message: message.into(),
            }),
            analysis_path: vec![AnalysisStep {
                callable: check.callable.clone(),
                location,
                message: "frozen callable contract comparison (abstract analysis)".into(),
            }],
            trace_witness: witness,
            ..Default::default()
        })),
    }))
}

fn descriptor(
    component: C,
    graph: &DescriptorGraph,
    header: &CallableHeaderInfo,
    expected: bool,
) -> Option<DescriptorDetail> {
    let row = match component {
        C::EscapingEffects => {
            if expected {
                header
                    .analysis
                    .effects
                    .declared_bound
                    .or(header.analysis.effects.escaping)
            } else {
                header.analysis.effects.escaping
            }
        }
        C::RequestedActions => header.analysis.effects.requested,
        C::CallableInterface | C::LatentBehavior => {
            return Some(DescriptorDetail::Type {
                graph: graph.clone(),
                ty: header.signature,
            });
        }
        C::ActionTrace | C::TemporalOutcomes => {
            let trace = if component == C::ActionTrace {
                header.analysis.trace
            } else {
                header.analysis.outcomes
            }?;
            return Some(DescriptorDetail::Trace {
                graph: graph.clone(),
                trace,
            });
        }
        _ => None,
    }?;
    Some(DescriptorDetail::EffectRow {
        graph: graph.clone(),
        row,
    })
}

fn component_name(component: C) -> &'static str {
    match component {
        C::CallableInterface => "callable_interface",
        C::LatentBehavior => "latent_behavior",
        C::EscapingEffects => "escaping_effects",
        C::RequestedActions => "requested_actions",
        C::Determinism => "determinism",
        C::ActionTrace => "action_trace",
        C::TemporalOutcomes => "temporal_outcomes",
        C::ResidualChecks => "residual_checks",
    }
}
