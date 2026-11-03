//! Residual eligibility for checked trace conformances.
//!
//! This refines the static classification into the obligations that an exact
//! runtime monitor could discharge. It does not create a plan, prove backend
//! support, or authorize execution. An `Unknown` result is eligible only when
//! the missing fact is a concrete, observable runtime fact of an otherwise
//! fully known action set; missing metadata, unknown callables, open action
//! rows, unsupported clause forms and definite violations are never eligible.

use etas_code::residual::identity::ClauseId;
use etas_hir::HirItemId;

use super::analysis::{
    CheckedPermissionCoverage, CheckedTraceClauseDisposition, CheckedTracePermissionDisposition,
    CheckedTraceStaticDisposition, CheckedTraceUncertainty, ClauseOutcome, TraceSpecAnalysisOutput,
};
use super::static_dispositions::TraceStaticDispositions;
use crate::{EffectPipelineError, EffectUnit};

/// The exact runtime fact an eligible clause obligation depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TraceResidualCause {
    /// A known requested action may match a deny selector; the concrete action
    /// identity decides.
    DenialSelector,
    /// A known requested action may match an allow selector; the concrete
    /// action identity decides.
    PermissionSelector,
    /// The guard is ordered before the target, but only a correlated accepted
    /// guard outcome discharges the requirement.
    OutcomeEvidence,
    /// The static trace lost the relevant ordering of known actions.
    OrderedActions,
}

/// Why a statically unproven conformance is not a residual obligation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceResidualIneligibility {
    /// The conformance has a static conclusion; no residual is created.
    NotUnproven,
    /// Some alternative is definitely violated. A runtime OR branch must not
    /// accept a statically violated alternative, and dropping that branch would
    /// change the checked contract, so the conformance stays rejected.
    ViolatedAlternative,
    /// The checked source owner, spec instance or a clause identity is absent.
    MissingSourceIdentity,
    /// The interprocedural trace summary is absent or incomplete.
    IncompleteSummary,
    /// The body calls an unknown callable parameter.
    UnresolvedCalls,
    /// The requested action row is open or contains row variables.
    OpenRequestedActions,
    /// Permission coverage depends on unresolved requested actions.
    UnresolvedPermission,
    /// Limit clauses have no checked enforcement model.
    UnsupportedLimit,
    /// A clause outcome is missing or disagrees with the classification.
    MissingClauseFacts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceResidualClause {
    pub alternative_index: usize,
    pub clause_index: usize,
    pub clause: ClauseId,
    pub cause: TraceResidualCause,
}

/// The complete set of clause obligations for one conformance. Every
/// alternative is retained; each has at least one obligation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceResidualRequirement {
    type_fact_index: usize,
    item: HirItemId,
    clauses: Vec<TraceResidualClause>,
}

impl TraceResidualRequirement {
    pub fn type_fact_index(&self) -> usize {
        self.type_fact_index
    }
    pub fn item(&self) -> HirItemId {
        self.item
    }
    pub fn clauses(&self) -> &[TraceResidualClause] {
        &self.clauses
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceResidualEligibility {
    Proven,
    Violated,
    NeedsConcrete(TraceResidualRequirement),
    Unsupported(TraceResidualIneligibility),
}

/// Classifies one checked conformance from the verified dispositions of the
/// same analysis run. The dispositions are revalidated against `analysis`.
pub fn classify_trace_residual(
    analysis: &TraceSpecAnalysisOutput,
    dispositions: &TraceStaticDispositions,
    type_fact_index: usize,
    item: HirItemId,
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<TraceResidualEligibility, EffectPipelineError> {
    use TraceResidualIneligibility as Ineligible;
    let checkpoint = || -> Result<(), EffectPipelineError> {
        if let Some(control) = control {
            control
                .checkpoint()
                .map_err(EffectPipelineError::Interrupted)?;
        }
        Ok(())
    };
    let conformance =
        dispositions.verified_conformance(type_fact_index, item, analysis, control)?;
    match conformance.disposition() {
        CheckedTraceStaticDisposition::Proven => return Ok(TraceResidualEligibility::Proven),
        CheckedTraceStaticDisposition::Violated => return Ok(TraceResidualEligibility::Violated),
        CheckedTraceStaticDisposition::Unsupported => {}
    }
    let unsupported = |reason| Ok(TraceResidualEligibility::Unsupported(reason));
    let source = conformance.source();
    if source.source_owner.is_none() || source.source_instance.is_none() {
        return unsupported(Ineligible::MissingSourceIdentity);
    }
    let Some(summary) = analysis
        .summaries
        .get(EffectUnit::Item(item))
        .filter(|summary| !summary.incomplete)
    else {
        return unsupported(Ineligible::IncompleteSummary);
    };
    if summary.action_trace.has_parameter_calls() {
        return unsupported(Ineligible::UnresolvedCalls);
    }
    if row_is_open(&summary.requested_actions) {
        return unsupported(Ineligible::OpenRequestedActions);
    }
    let outcomes = analysis
        .clause_outcomes
        .get(&item)
        .and_then(|items| items.get(source.conformance_position()));
    let mut clauses = Vec::new();
    for (alternative_index, alternative) in conformance.alternatives().iter().enumerate() {
        checkpoint()?;
        let ids = source
            .clause_ids
            .get(alternative_index)
            .ok_or_else(invalid_sites)?;
        let first = clauses.len();
        let permission = alternative.permission_evidence();
        match alternative.permission() {
            CheckedTracePermissionDisposition::Proven => {}
            CheckedTracePermissionDisposition::Violated => {
                return unsupported(Ineligible::ViolatedAlternative);
            }
            CheckedTracePermissionDisposition::Unsupported => {
                if permission.unresolved() || permission.unresolved_calls() {
                    return unsupported(Ineligible::UnresolvedPermission);
                }
                if row_is_open(permission.requested()) {
                    return unsupported(Ineligible::OpenRequestedActions);
                }
                if permission.uncovered().is_empty()
                    || permission
                        .uncovered()
                        .iter()
                        .any(|action| action.coverage() != CheckedPermissionCoverage::Undetermined)
                {
                    return unsupported(Ineligible::MissingClauseFacts);
                }
                // Coverage is decided by the complete allow set of this
                // alternative, so each allow clause is an obligation site.
                let mut allow_sites = 0usize;
                for (clause_index, fact) in alternative.clauses().iter().enumerate() {
                    if fact.disposition == CheckedTraceClauseDisposition::AlternativePermission {
                        let clause = match ids.get(clause_index) {
                            Some(Some(id)) if fact.source_clause == Some(*id) => *id,
                            Some(None) => return unsupported(Ineligible::MissingSourceIdentity),
                            _ => return Err(invalid_sites()),
                        };
                        allow_sites += 1;
                        clauses.push(TraceResidualClause {
                            alternative_index,
                            clause_index,
                            clause,
                            cause: TraceResidualCause::PermissionSelector,
                        });
                    }
                }
                if allow_sites == 0 {
                    return unsupported(Ineligible::MissingClauseFacts);
                }
            }
        }
        for (clause_index, fact) in alternative.clauses().iter().enumerate() {
            checkpoint()?;
            let cause = match fact.disposition {
                CheckedTraceClauseDisposition::Proven
                | CheckedTraceClauseDisposition::AlternativePermission => continue,
                CheckedTraceClauseDisposition::Violated => {
                    return unsupported(Ineligible::ViolatedAlternative);
                }
                CheckedTraceClauseDisposition::Unsupported(uncertainty) => match uncertainty {
                    CheckedTraceUncertainty::UnsupportedLimit => {
                        return unsupported(Ineligible::UnsupportedLimit);
                    }
                    CheckedTraceUncertainty::OpenRequestedActions => {
                        return unsupported(Ineligible::OpenRequestedActions);
                    }
                    CheckedTraceUncertainty::DenialSelector => {
                        let Some(Some(ClauseOutcome::Denial(outcome))) = outcomes
                            .and_then(|alternatives| alternatives.get(alternative_index))
                            .and_then(|clauses| clauses.get(clause_index))
                        else {
                            return unsupported(Ineligible::MissingClauseFacts);
                        };
                        if outcome.source_clause != fact.source_clause {
                            return unsupported(Ineligible::MissingClauseFacts);
                        }
                        if outcome.unresolved_calls {
                            return unsupported(Ineligible::UnresolvedCalls);
                        }
                        if row_is_open(&outcome.requested) {
                            return unsupported(Ineligible::OpenRequestedActions);
                        }
                        TraceResidualCause::DenialSelector
                    }
                    CheckedTraceUncertainty::OutcomeEvidence => TraceResidualCause::OutcomeEvidence,
                    CheckedTraceUncertainty::OrderedActions => TraceResidualCause::OrderedActions,
                },
            };
            let clause = match ids.get(clause_index) {
                Some(Some(id)) if fact.source_clause == Some(*id) => *id,
                Some(None) => return unsupported(Ineligible::MissingSourceIdentity),
                _ => return Err(invalid_sites()),
            };
            clauses.push(TraceResidualClause {
                alternative_index,
                clause_index,
                clause,
                cause,
            });
        }
        // An Unsupported conformance with no Violated and no Proven
        // alternative must leave an obligation in every alternative.
        if clauses.len() == first {
            return unsupported(Ineligible::MissingClauseFacts);
        }
    }
    Ok(TraceResidualEligibility::NeedsConcrete(
        TraceResidualRequirement {
            type_fact_index,
            item,
            clauses,
        },
    ))
}

fn row_is_open(row: &crate::EffectRow) -> bool {
    row.open.is_some()
        || row
            .effects
            .iter()
            .any(|effect| matches!(effect, crate::Effect::Var(_)))
}

fn invalid_sites() -> EffectPipelineError {
    EffectPipelineError::InvalidTraceSpecSourceSites {
        reason: "residual eligibility does not match its checked source conformance".into(),
    }
}
