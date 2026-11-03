use etas_hir_analysis::interprocedural::{
    InterproceduralAnalysis, InterproceduralAnalysisResult, SummaryStore,
};
use std::{collections::BTreeMap, sync::Arc};

use crate::{EffectOutput, EffectPipelineError, EffectRegistry, EffectUnit};

use super::model::{TraceSpecClause, TraceSpecModelStore};
use super::monitor::{TemporalMonitorResult, require_before};
use super::semantics::TraceSpecSemantics;
use super::validate::reject_item_for_trace_spec;
use crate::diagnostic_anchor::{DiagnosticAnchor, materialize_effect_diagnostic};

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub struct TraceSpecAnalysisOutput {
    pub summaries: SummaryStore<EffectUnit, super::domain::TraceSpecSummary>,
    pub(crate) source_conformances: BTreeMap<usize, CheckedTraceSourceConformance>,
    /// Positions are session-local; the optional clause identity ties each
    /// outcome to its source when a checked source snapshot is available.
    pub(crate) clause_outcomes: BTreeMap<etas_hir::HirItemId, ClauseConformanceOutcomes>,
    pub(crate) permission_outcomes: BTreeMap<etas_hir::HirItemId, PermissionConformanceOutcomes>,
    pub(super) external_outcomes:
        BTreeMap<super::model::ExternalTraceSpecItem, ExternalTraceOutcome>,
}

/// Session-local lookup into already checked HIR/type facts. Neither this
/// index nor a source instance proves monitor coverage or installation.
#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct CheckedTraceSourceConformance {
    pub item: etas_hir::HirItemId,
    pub span: etas_core::Span,
    pub source_owner: Option<super::model::CheckedTraceSourceOwner>,
    pub source_instance: Option<etas_code::residual::identity::SourceSpecInstance>,
    pub clause_ids: Vec<Vec<Option<etas_code::residual::identity::ClauseId>>>,
    // The exact materialized rules analyzed for this conformance. Sharing the
    // snapshot keeps subsequent classification clones from copying the model.
    pub(super) analyzed_alternatives: Arc<super::model::TraceSpecClauseAlternatives>,
    conformance_position: usize,
    clause_kinds: Vec<Vec<SourceClauseKind>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, etas_core::RetainedSize)]
enum SourceClauseKind {
    Allow {
        permission_index: usize,
        origin_index: usize,
    },
    Deny,
    Temporal,
    Limit,
}

/// Static analysis only. In particular, `Unsupported` is not evidence that a
/// runtime residual predicate or an installation exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckedTraceClauseDisposition {
    Proven,
    Violated,
    Unsupported(CheckedTraceUncertainty),
    /// Permission clauses are analyzed together for an entire alternative.
    AlternativePermission,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckedTraceUncertainty {
    DenialSelector,
    OutcomeEvidence,
    OrderedActions,
    OpenRequestedActions,
    UnsupportedLimit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckedTraceClauseFact {
    pub source_clause: Option<etas_code::residual::identity::ClauseId>,
    pub disposition: CheckedTraceClauseDisposition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckedTracePermissionDisposition {
    Proven,
    Violated,
    Unsupported,
}

/// A static result for a complete alternative or conformance. Unsupported
/// means that no conclusion is available; it is not an installed guard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckedTraceStaticDisposition {
    Proven,
    Violated,
    Unsupported,
}

/// Permission analysis applies to a complete alternative, not an individual
/// allow clause. `uncovered` retains whether each selector match is unknown.
pub struct CheckedTracePermissionFact<'a> {
    pub requested: &'a crate::EffectRow,
    uncovered: &'a [(crate::Effect, bool)],
    pub unresolved: bool,
    pub unresolved_calls: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckedPermissionCoverage {
    Uncovered,
    Undetermined,
}

#[derive(Clone, Copy, Debug)]
pub struct CheckedUncoveredAction<'a> {
    pub action: &'a crate::Effect,
    pub coverage: CheckedPermissionCoverage,
}

impl CheckedTracePermissionFact<'_> {
    pub fn uncovered(&self) -> impl ExactSizeIterator<Item = CheckedUncoveredAction<'_>> {
        self.uncovered
            .iter()
            .map(|(action, uncertain)| CheckedUncoveredAction {
                action,
                coverage: if *uncertain {
                    CheckedPermissionCoverage::Undetermined
                } else {
                    CheckedPermissionCoverage::Uncovered
                },
            })
    }

    pub fn disposition(&self) -> CheckedTracePermissionDisposition {
        if self.uncovered.iter().any(|(_, uncertain)| !uncertain) {
            CheckedTracePermissionDisposition::Violated
        } else if self.unresolved || !self.uncovered.is_empty() {
            CheckedTracePermissionDisposition::Unsupported
        } else {
            CheckedTracePermissionDisposition::Proven
        }
    }
}

impl CheckedTraceSourceConformance {
    /// Position of this conformance among its owner's checked conformances.
    pub(crate) fn conformance_position(&self) -> usize {
        self.conformance_position
    }
}

impl TraceSpecAnalysisOutput {
    /// Combines all conjuncts of one checked alternative without flattening OR.
    /// Every clause is visited even after a violation, so missing facts cannot
    /// be hidden behind an earlier conclusion.
    pub fn checked_alternative_disposition(
        &self,
        type_fact_index: usize,
        alternative_index: usize,
        control: Option<&dyn etas_utils::PipelineExecutionControl>,
    ) -> Result<CheckedTraceStaticDisposition, EffectPipelineError> {
        let source = self
            .source_conformances
            .get(&type_fact_index)
            .ok_or_else(|| EffectPipelineError::InvalidTraceSpecSourceSites {
                reason: "trace alternative has no checked source conformance".into(),
            })?;
        let clauses = source.clause_ids.get(alternative_index).ok_or_else(|| {
            EffectPipelineError::InvalidTraceSpecSourceSites {
                reason: "trace alternative index is not part of its checked conformance".into(),
            }
        })?;
        if clauses.is_empty() {
            return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                reason: "checked trace alternative is empty".into(),
            });
        }
        if clauses.len() > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
            return Err(EffectPipelineError::AnalysisLimitExceeded {
                resource: "trace alternative dispositions",
                limit: etas_package_metadata::MAX_METADATA_GRAPH_NODES,
            });
        }
        let mut violated = false;
        let mut unsupported = false;
        match self
            .checked_permission_fact(type_fact_index, alternative_index)?
            .disposition()
        {
            CheckedTracePermissionDisposition::Proven => {}
            CheckedTracePermissionDisposition::Violated => violated = true,
            CheckedTracePermissionDisposition::Unsupported => unsupported = true,
        }
        for clause_index in 0..clauses.len() {
            if let Some(control) = control {
                control
                    .checkpoint()
                    .map_err(EffectPipelineError::Interrupted)?;
            }
            match self
                .checked_clause_fact(type_fact_index, alternative_index, clause_index)?
                .disposition
            {
                CheckedTraceClauseDisposition::Proven
                | CheckedTraceClauseDisposition::AlternativePermission => {}
                CheckedTraceClauseDisposition::Violated => violated = true,
                CheckedTraceClauseDisposition::Unsupported(_) => unsupported = true,
            }
        }
        Ok(if violated {
            CheckedTraceStaticDisposition::Violated
        } else if unsupported {
            CheckedTraceStaticDisposition::Unsupported
        } else {
            CheckedTraceStaticDisposition::Proven
        })
    }

    /// OR composition: a proven alternative suffices; a violation requires
    /// every alternative to be definitely violated.
    pub fn checked_conformance_disposition(
        &self,
        type_fact_index: usize,
        control: Option<&dyn etas_utils::PipelineExecutionControl>,
    ) -> Result<CheckedTraceStaticDisposition, EffectPipelineError> {
        let source = self
            .source_conformances
            .get(&type_fact_index)
            .ok_or_else(|| EffectPipelineError::InvalidTraceSpecSourceSites {
                reason: "trace conformance has no checked source analysis".into(),
            })?;
        if source.clause_ids.is_empty() {
            return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                reason: "checked trace conformance has no alternatives".into(),
            });
        }
        let limit = etas_package_metadata::MAX_METADATA_GRAPH_NODES;
        let mut remaining = limit;
        let mut proven = false;
        let mut unsupported = false;
        for (index, clauses) in source.clause_ids.iter().enumerate() {
            remaining = remaining
                .checked_sub(clauses.len().saturating_add(1))
                .ok_or(EffectPipelineError::AnalysisLimitExceeded {
                    resource: "trace conformance dispositions",
                    limit,
                })?;
            match self.checked_alternative_disposition(type_fact_index, index, control)? {
                CheckedTraceStaticDisposition::Proven => proven = true,
                CheckedTraceStaticDisposition::Unsupported => unsupported = true,
                CheckedTraceStaticDisposition::Violated => {}
            }
        }
        Ok(if proven {
            CheckedTraceStaticDisposition::Proven
        } else if unsupported {
            CheckedTraceStaticDisposition::Unsupported
        } else {
            CheckedTraceStaticDisposition::Violated
        })
    }

    pub fn source_conformance(
        &self,
        type_fact_index: usize,
    ) -> Option<&CheckedTraceSourceConformance> {
        self.source_conformances.get(&type_fact_index)
    }

    pub fn checked_permission_fact(
        &self,
        type_fact_index: usize,
        alternative_index: usize,
    ) -> Result<CheckedTracePermissionFact<'_>, EffectPipelineError> {
        let invalid = || EffectPipelineError::InvalidTraceSpecSourceSites {
            reason: "trace permission analysis does not match its checked source alternative"
                .into(),
        };
        let source = self
            .source_conformances
            .get(&type_fact_index)
            .ok_or_else(invalid)?;
        source
            .clause_ids
            .get(alternative_index)
            .ok_or_else(invalid)?;
        let permission = self
            .permission_outcomes
            .get(&source.item)
            .and_then(|items| items.get(source.conformance_position))
            .and_then(|alternatives| alternatives.get(alternative_index))
            .and_then(Option::as_ref)
            .ok_or_else(invalid)?;
        Ok(CheckedTracePermissionFact {
            requested: &permission.requested,
            uncovered: &permission.uncovered,
            unresolved: permission.unresolved,
            unresolved_calls: permission.unresolved_calls,
        })
    }

    /// Retrieve an actual analyzed conclusion, tied to the exact checked
    /// conformance and alternative. This does not establish residual coverage.
    pub fn checked_clause_fact(
        &self,
        type_fact_index: usize,
        alternative_index: usize,
        clause_index: usize,
    ) -> Result<CheckedTraceClauseFact, EffectPipelineError> {
        let invalid = || EffectPipelineError::InvalidTraceSpecSourceSites {
            reason: "trace clause analysis does not match its checked source conformance".into(),
        };
        let source = self
            .source_conformances
            .get(&type_fact_index)
            .ok_or_else(invalid)?;
        let kind = source
            .clause_kinds
            .get(alternative_index)
            .and_then(|clauses| clauses.get(clause_index))
            .ok_or_else(invalid)?;
        let source_clause = *source
            .clause_ids
            .get(alternative_index)
            .and_then(|clauses| clauses.get(clause_index))
            .ok_or_else(invalid)?;
        let result = self
            .clause_outcomes
            .get(&source.item)
            .and_then(|items| items.get(source.conformance_position))
            .and_then(|alternatives| alternatives.get(alternative_index))
            .and_then(|clauses| clauses.get(clause_index))
            .ok_or_else(invalid)?;
        let disposition = match (kind, result) {
            (SourceClauseKind::Deny, Some(ClauseOutcome::Denial(outcome)))
                if outcome.source_clause == source_clause =>
            {
                match outcome.disposition {
                    DenialStaticDisposition::Proven => CheckedTraceClauseDisposition::Proven,
                    DenialStaticDisposition::Violated => CheckedTraceClauseDisposition::Violated,
                    DenialStaticDisposition::Unsupported => {
                        CheckedTraceClauseDisposition::Unsupported(
                            CheckedTraceUncertainty::DenialSelector,
                        )
                    }
                }
            }
            (SourceClauseKind::Temporal, Some(ClauseOutcome::Temporal(outcome)))
                if outcome.source_clause == source_clause =>
            {
                match outcome.disposition {
                    TemporalStaticDisposition::Proven => CheckedTraceClauseDisposition::Proven,
                    TemporalStaticDisposition::Violated => CheckedTraceClauseDisposition::Violated,
                    TemporalStaticDisposition::Unsupported(reason) => {
                        CheckedTraceClauseDisposition::Unsupported(match reason {
                            TemporalUncertainty::OutcomeEvidence => {
                                CheckedTraceUncertainty::OutcomeEvidence
                            }
                            TemporalUncertainty::OrderedActions => {
                                CheckedTraceUncertainty::OrderedActions
                            }
                            TemporalUncertainty::OpenRequestedActions => {
                                CheckedTraceUncertainty::OpenRequestedActions
                            }
                        })
                    }
                }
            }
            (
                SourceClauseKind::Allow {
                    permission_index,
                    origin_index,
                },
                None,
            ) => {
                let permission = self
                    .permission_outcomes
                    .get(&source.item)
                    .and_then(|items| items.get(source.conformance_position))
                    .and_then(|alternatives| alternatives.get(alternative_index))
                    .and_then(Option::as_ref)
                    .ok_or_else(invalid)?;
                let expected_origin = *source
                    .clause_ids
                    .get(alternative_index)
                    .and_then(|clauses| clauses.get(*origin_index))
                    .ok_or_else(invalid)?;
                let Some((_, (index, _), id)) = permission.permissions.get(*permission_index)
                else {
                    return Err(invalid());
                };
                if index != origin_index || *id != expected_origin {
                    return Err(invalid());
                }
                CheckedTraceClauseDisposition::AlternativePermission
            }
            (SourceClauseKind::Limit, None) => CheckedTraceClauseDisposition::Unsupported(
                CheckedTraceUncertainty::UnsupportedLimit,
            ),
            _ => return Err(invalid()),
        };
        Ok(CheckedTraceClauseFact {
            source_clause,
            disposition,
        })
    }

    pub(super) fn validate_source_conformances(
        &self,
        models: &TraceSpecModelStore,
        types: &etas_types::TypeOutput,
        effects: &EffectOutput,
    ) -> Result<(), EffectPipelineError> {
        if self.source_conformances != collect_source_conformances(models, types, effects, None)? {
            return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                reason: "trace source index does not match checked conformance facts".into(),
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, etas_core::RetainedSize)]
pub(super) enum ExternalTraceOutcome {
    Invalid,
    Ready {
        span: etas_core::Span,
        trace: Box<super::validate::ExternalTraceSummary>,
        clauses: ClauseConformanceOutcomes,
        permissions: PermissionConformanceOutcomes,
    },
}

pub struct TraceSpecAnalysisInput<'a> {
    pub source_snapshots: &'a [crate::EffectSourceSnapshot<'a>],
    pub external_trace_specs:
        &'a [crate::AnchoredExternalMetadata<crate::ExternalTraceSpecSummaryMetadata>],
    pub hir: &'a etas_hir::HirRevision,
    pub types: &'a etas_types::TypeOutput,
    pub registry: &'a EffectRegistry,
    pub models: &'a TraceSpecModelStore,
    pub units: &'a [EffectUnit],
    pub external_summaries:
        &'a [crate::AnchoredExternalMetadata<crate::ExternalEffectSummaryMetadata>],
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub(crate) struct PermissionOutcome {
    pub permissions: Vec<(
        super::model::TraceSpecPattern,
        (usize, etas_core::Span),
        Option<etas_code::residual::identity::ClauseId>,
    )>,
    pub requested: Arc<crate::EffectRow>,
    pub unresolved_calls: bool,
    pub uncovered: Vec<(crate::Effect, bool)>,
    pub unresolved: bool,
}

pub(crate) type PermissionConformanceOutcomes = Vec<Vec<Option<PermissionOutcome>>>;

pub(crate) fn analyze_permissions(
    clauses: &[TraceSpecClause],
    permissions: &[(&super::model::TraceSpecPattern, (usize, etas_core::Span))],
    requested: Arc<crate::EffectRow>,
    unresolved_calls: bool,
    registry: &EffectRegistry,
    types: &etas_types::TypeOutput,
) -> Option<PermissionOutcome> {
    let mut remaining = etas_package_metadata::MAX_METADATA_GRAPH_NODES;
    let mut uncovered = Vec::new();
    if !permissions.is_empty() {
        for action in requested.effects.iter() {
            let mut matches = false;
            for (pattern, _) in permissions {
                remaining = remaining.checked_sub(1)?;
                if pattern.matches_effect(action, registry, types) {
                    matches = true;
                    break;
                }
            }
            if !matches {
                let mut uncertain = false;
                for (pattern, _) in permissions {
                    remaining = remaining.checked_sub(1)?;
                    if pattern.may_match_effect(action, registry, types) {
                        uncertain = true;
                        break;
                    }
                }
                uncovered.push((action.clone(), uncertain));
            }
        }
    }
    Some(PermissionOutcome {
        permissions: permissions
            .iter()
            .map(|(pattern, origin)| {
                Some((
                    (*pattern).clone(),
                    *origin,
                    clauses.get(origin.0)?.source_clause(),
                ))
            })
            .collect::<Option<Vec<_>>>()?,
        unresolved: !permissions.is_empty() && (requested.open.is_some() || unresolved_calls),
        requested,
        unresolved_calls,
        uncovered,
    })
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub(crate) struct TemporalClauseOutcome {
    pub source_clause: Option<etas_code::residual::identity::ClauseId>,
    pub guard: super::model::TraceSpecPattern,
    pub target: super::model::TraceSpecPattern,
    pub disposition: TemporalStaticDisposition,
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub(crate) struct DenialClauseOutcome {
    pub source_clause: Option<etas_code::residual::identity::ClauseId>,
    pub pattern: super::model::TraceSpecPattern,
    pub requested: Arc<crate::EffectRow>,
    pub unresolved_calls: bool,
    pub disposition: DenialStaticDisposition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub(crate) enum DenialStaticDisposition {
    Proven,
    Violated,
    Unsupported,
}

pub(crate) fn analyze_denial(
    pattern: &super::model::TraceSpecPattern,
    trace: &crate::ActionTraceDomain,
    requested: &crate::EffectRow,
    registry: &EffectRegistry,
    types: &etas_types::TypeOutput,
) -> DenialStaticDisposition {
    if requested
        .effects
        .iter()
        .any(|action| pattern.matches_effect(action, registry, types))
    {
        DenialStaticDisposition::Violated
    } else if requested.open.is_some()
        || trace.has_parameter_calls()
        || requested
            .effects
            .iter()
            .any(|action| pattern.may_match_effect(action, registry, types))
    {
        DenialStaticDisposition::Unsupported
    } else {
        DenialStaticDisposition::Proven
    }
}

#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub(crate) enum ClauseOutcome {
    Temporal(TemporalClauseOutcome),
    Denial(DenialClauseOutcome),
}

impl ClauseOutcome {
    pub fn matches_clause(&self, clause: &TraceSpecClause) -> bool {
        match (self, clause) {
            (Self::Temporal(outcome), _) => outcome.matches_clause(clause),
            (Self::Denial(outcome), TraceSpecClause::Deny { pattern, .. }) => {
                outcome.source_clause == clause.source_clause() && &outcome.pattern == pattern
            }
            _ => false,
        }
    }
}

/// A static conclusion for this checked model only. Unsupported results are
/// not obligations or evidence of runtime installability.
#[derive(Clone, Copy, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub(crate) enum TemporalStaticDisposition {
    Proven,
    Violated,
    Unsupported(TemporalUncertainty),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub(crate) enum TemporalUncertainty {
    OutcomeEvidence,
    OrderedActions,
    OpenRequestedActions,
}

impl TemporalStaticDisposition {
    pub fn from_monitor(result: TemporalMonitorResult, requested_actions_open: bool) -> Self {
        match result {
            TemporalMonitorResult::Satisfied if requested_actions_open => {
                Self::Unsupported(TemporalUncertainty::OpenRequestedActions)
            }
            TemporalMonitorResult::Satisfied => Self::Proven,
            TemporalMonitorResult::Violation => Self::Violated,
            TemporalMonitorResult::NeedsOutcomeEvidence => {
                Self::Unsupported(TemporalUncertainty::OutcomeEvidence)
            }
            TemporalMonitorResult::Unknown => {
                Self::Unsupported(TemporalUncertainty::OrderedActions)
            }
        }
    }
}

impl TemporalClauseOutcome {
    pub fn matches_clause(&self, clause: &TraceSpecClause) -> bool {
        if self.source_clause != clause.source_clause() {
            return false;
        }
        match clause {
            TraceSpecClause::RequireBefore { guard, target, .. } => {
                &self.guard == guard && &self.target == target
            }
            TraceSpecClause::RequireAfter {
                target, obligation, ..
            } => &self.guard == target && &self.target == obligation,
            _ => false,
        }
    }
}

pub(crate) type ClauseConformanceOutcomes = Vec<Vec<Vec<Option<ClauseOutcome>>>>;

pub fn analyze_trace_spec_monitors(
    input: TraceSpecAnalysisInput<'_>,
    effects: &mut EffectOutput,
) -> Result<TraceSpecAnalysisOutput, EffectPipelineError> {
    analyze_with_control(input, effects, None)
}

pub fn analyze_trace_spec_monitors_controlled(
    input: TraceSpecAnalysisInput<'_>,
    effects: &mut EffectOutput,
    control: &dyn etas_utils::PipelineExecutionControl,
) -> Result<TraceSpecAnalysisOutput, EffectPipelineError> {
    analyze_with_control(input, effects, Some(control))
}

fn analyze_with_control(
    input: TraceSpecAnalysisInput<'_>,
    effects: &mut EffectOutput,
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<TraceSpecAnalysisOutput, EffectPipelineError> {
    let TraceSpecAnalysisInput {
        source_snapshots,
        external_trace_specs,
        hir,
        types,
        registry,
        models,
        units,
        external_summaries,
    } = input;
    models.validate_source_sites(hir, types, registry, external_trace_specs, source_snapshots)?;
    super::materialize::validate_conformance_instances(
        hir,
        types,
        registry,
        source_snapshots,
        models,
        &effects.facts.trace_specs,
        &effects.diagnostics,
    )?;
    let source_conformances = collect_source_conformances(models, types, effects, control)?;
    let trace_spec_items = models.referenced_by_item.keys().copied().collect();
    let analysis = InterproceduralAnalysis::new(
        units.iter().copied(),
        TraceSpecSemantics::new(hir, types, registry, &effects.facts, trace_spec_items),
    );
    let result = match control {
        Some(control) => analysis
            .solve_controlled(control)
            .map_err(EffectPipelineError::Interrupted)?,
        None => analysis.solve(),
    };
    let InterproceduralAnalysisResult {
        semantics,
        summaries,
        convergence,
        diagnostics: analysis_diagnostics,
        ..
    } = result;
    let (diagnostics, rejected_items, diagnostic_materialization_errors) = semantics.into_parts();
    if let Some(error) = diagnostic_materialization_errors.into_iter().next() {
        return Err(error);
    }
    effects.diagnostics.extend(diagnostics);
    for diagnostic in analysis_diagnostics {
        let anchor = match &diagnostic {
            etas_hir_analysis::interprocedural::InterproceduralDiagnostic::InvalidCondensationGraph => DiagnosticAnchor::Project,
            etas_hir_analysis::interprocedural::InterproceduralDiagnostic::MissingBody { unit } => DiagnosticAnchor::Unit(*unit),
            etas_hir_analysis::interprocedural::InterproceduralDiagnostic::MissingSummary { call, .. } => DiagnosticAnchor::Expr(*call),
            etas_hir_analysis::interprocedural::InterproceduralDiagnostic::IterationLimitReached { component } => component.first().copied().map(DiagnosticAnchor::Unit).unwrap_or(DiagnosticAnchor::Project),
        };
        effects.diagnostics.push(materialize_effect_diagnostic(
            hir,
            etas_core::EffectDiagnosticCode::IncompleteEffectFacts,
            anchor,
            format!("trace spec interprocedural analysis failed: {diagnostic:?}"),
        )?);
    }
    for convergence in convergence {
        if convergence.status == etas_utils::ConvergenceStatus::IterationLimitReached {
            for unit in &convergence.units {
                if let Some(owner) = unit.owner() {
                    reject_item_for_trace_spec(effects, owner);
                }
            }
            let anchor = convergence
                .units
                .first()
                .copied()
                .map(DiagnosticAnchor::Unit)
                .unwrap_or(DiagnosticAnchor::Project);
            effects.diagnostics.push(materialize_effect_diagnostic(
                hir,
                etas_core::EffectDiagnosticCode::IncompleteEffectFacts,
                anchor,
                "trace spec summary solving reached the iteration limit",
            )?);
        }
    }
    for item in rejected_items {
        reject_item_for_trace_spec(effects, item);
    }

    let clause_outcomes = collect_clause_outcomes(models, &summaries, registry, types, control)?;
    let permission_outcomes =
        collect_permission_outcomes(models, &summaries, effects, registry, types, control)?;
    let external_outcomes = collect_external_outcomes(
        models,
        external_summaries,
        effects,
        registry,
        types,
        control,
    )?;
    Ok(TraceSpecAnalysisOutput {
        summaries,
        source_conformances,
        clause_outcomes,
        permission_outcomes,
        external_outcomes,
    })
}

fn collect_source_conformances(
    models: &TraceSpecModelStore,
    types: &etas_types::TypeOutput,
    effects: &EffectOutput,
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<BTreeMap<usize, CheckedTraceSourceConformance>, EffectPipelineError> {
    let limit = etas_package_metadata::MAX_METADATA_GRAPH_NODES;
    let mut remaining = limit;
    let mut recorded = std::collections::HashMap::new();
    for (item, conformances) in &effects.facts.trace_specs.conformances {
        for conformance in conformances {
            if let Some(control) = control {
                control
                    .checkpoint()
                    .map_err(EffectPipelineError::Interrupted)?;
            }
            remaining =
                remaining
                    .checked_sub(1)
                    .ok_or(EffectPipelineError::AnalysisLimitExceeded {
                        resource: "trace source conformance index",
                        limit,
                    })?;
            if recorded
                .insert((*item, conformance.span), conformance)
                .is_some()
            {
                return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                    reason: "duplicate trace conformance in checked source index".into(),
                });
            }
        }
    }
    let mut result = BTreeMap::new();
    for (item, indices) in &models.referenced_fact_indices {
        let alternatives = models.referenced_by_item.get(item).ok_or_else(|| {
            EffectPipelineError::InvalidTraceSpecSourceSites {
                reason: "trace source index has no checked alternatives".into(),
            }
        })?;
        if indices.len() != alternatives.len() {
            return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                reason: "trace source index has inconsistent conformance counts".into(),
            });
        }
        for (conformance_position, (index, groups)) in indices.iter().zip(alternatives).enumerate()
        {
            if let Some(control) = control {
                control
                    .checkpoint()
                    .map_err(EffectPipelineError::Interrupted)?;
            }
            remaining =
                remaining
                    .checked_sub(1)
                    .ok_or(EffectPipelineError::AnalysisLimitExceeded {
                        resource: "trace source conformance index",
                        limit,
                    })?;
            let fact = types
                .facts
                .trace_spec_conformances
                .get(*index)
                .ok_or_else(|| EffectPipelineError::InvalidTraceSpecSourceSites {
                    reason: "trace source index refers to an invalid type fact".into(),
                })?;
            if fact.item != *item {
                return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                    reason: "trace source index has a mismatched conformance owner".into(),
                });
            }
            let stored = recorded.get(&(fact.item, fact.span)).ok_or_else(|| {
                EffectPipelineError::InvalidTraceSpecSourceSites {
                    reason: "trace source index has no materialized conformance".into(),
                }
            })?;
            let source_owner = models.source_owners.get(item).ok_or_else(|| {
                EffectPipelineError::InvalidTraceSpecSourceSites {
                    reason: "trace source index has no checked owner record".into(),
                }
            })?;
            if let Some(instance) = &stored.source_spec_instance {
                let size = instance
                    .arguments()
                    .len()
                    .saturating_add(instance.structural_path().len());
                remaining = remaining.checked_sub(size).ok_or(
                    EffectPipelineError::AnalysisLimitExceeded {
                        resource: "trace source conformance index",
                        limit,
                    },
                )?;
            }
            remaining = remaining.checked_sub(groups.len()).ok_or(
                EffectPipelineError::AnalysisLimitExceeded {
                    resource: "trace source conformance index",
                    limit,
                },
            )?;
            let mut clause_ids = Vec::with_capacity(groups.len());
            let mut clause_kinds = Vec::with_capacity(groups.len());
            for group in groups {
                if let Some(control) = control {
                    control
                        .checkpoint()
                        .map_err(EffectPipelineError::Interrupted)?;
                }
                let entries = group.len().checked_mul(2).ok_or(
                    EffectPipelineError::AnalysisLimitExceeded {
                        resource: "trace source conformance index",
                        limit,
                    },
                )?;
                remaining = remaining.checked_sub(entries).ok_or(
                    EffectPipelineError::AnalysisLimitExceeded {
                        resource: "trace source conformance index",
                        limit,
                    },
                )?;
                clause_ids.push(group.iter().map(TraceSpecClause::source_clause).collect());
                let normalized = super::model::normalize_alternative(group).map_err(|_| {
                    EffectPipelineError::AnalysisLimitExceeded {
                        resource: "trace source conformance index",
                        limit,
                    }
                })?;
                remaining = remaining.checked_sub(normalized.permissions.len()).ok_or(
                    EffectPipelineError::AnalysisLimitExceeded {
                        resource: "trace source conformance index",
                        limit,
                    },
                )?;
                let permissions = normalized
                    .permissions
                    .iter()
                    .enumerate()
                    .map(|(position, (pattern, (origin, _)))| (*pattern, (position, *origin)))
                    .collect::<BTreeMap<_, _>>();
                let mut kinds = Vec::with_capacity(group.len());
                for clause in group {
                    if let Some(control) = control {
                        control
                            .checkpoint()
                            .map_err(EffectPipelineError::Interrupted)?;
                    }
                    kinds.push(match clause {
                        TraceSpecClause::Allow { pattern, .. } => {
                            let (permission_index, origin_index) =
                                permissions.get(pattern).copied().ok_or_else(|| {
                                    EffectPipelineError::InvalidTraceSpecSourceSites {
                                        reason: "normalized trace permission is missing".into(),
                                    }
                                })?;
                            SourceClauseKind::Allow {
                                permission_index,
                                origin_index,
                            }
                        }
                        TraceSpecClause::Deny { .. } => SourceClauseKind::Deny,
                        TraceSpecClause::RequireBefore { .. }
                        | TraceSpecClause::RequireAfter { .. } => SourceClauseKind::Temporal,
                        TraceSpecClause::Limit { .. } => SourceClauseKind::Limit,
                    });
                }
                clause_kinds.push(kinds);
            }
            let entry = CheckedTraceSourceConformance {
                item: *item,
                span: fact.span,
                source_owner: source_owner.clone(),
                source_instance: stored.source_spec_instance.clone(),
                clause_ids,
                analyzed_alternatives: Arc::new(groups.clone()),
                conformance_position,
                clause_kinds,
            };
            if result.insert(*index, entry).is_some() {
                return Err(EffectPipelineError::InvalidTraceSpecSourceSites {
                    reason: "duplicate type fact in trace source index".into(),
                });
            }
        }
    }
    Ok(result)
}

fn collect_permission_outcomes(
    models: &TraceSpecModelStore,
    summaries: &SummaryStore<EffectUnit, super::domain::TraceSpecSummary>,
    effects: &EffectOutput,
    registry: &EffectRegistry,
    types: &etas_types::TypeOutput,
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<BTreeMap<etas_hir::HirItemId, PermissionConformanceOutcomes>, EffectPipelineError> {
    let mut output = BTreeMap::new();
    for (item, conformances) in &models.referenced_by_item {
        let Some(summary) = summaries
            .get(EffectUnit::Item(*item))
            .filter(|summary| !summary.incomplete)
        else {
            continue;
        };
        let Some(effect_summary) = effects.facts.item_effects.get(item) else {
            continue;
        };
        let requested = Arc::new(
            crate::EffectCoverage {
                registry,
                types: &types.store,
            }
            .subtract_handled(&summary.requested_actions, &effect_summary.default_actions),
        );
        let unresolved = summary.action_trace.has_parameter_calls();
        output.insert(
            *item,
            analyze_permission_groups(
                conformances,
                requested,
                unresolved,
                registry,
                types,
                control,
            )?,
        );
    }
    Ok(output)
}

fn analyze_permission_groups(
    conformances: &super::model::TraceSpecConformances,
    requested: Arc<crate::EffectRow>,
    unresolved: bool,
    registry: &EffectRegistry,
    types: &etas_types::TypeOutput,
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<PermissionConformanceOutcomes, EffectPipelineError> {
    let mut item_outcomes = Vec::with_capacity(conformances.len());
    for alternatives in conformances {
        let mut outcomes = Vec::with_capacity(alternatives.len());
        for clauses in alternatives {
            if let Some(control) = control {
                control
                    .checkpoint()
                    .map_err(EffectPipelineError::Interrupted)?;
            }
            let outcome =
                super::model::normalize_alternative(clauses)
                    .ok()
                    .and_then(|normalized| {
                        analyze_permissions(
                            clauses,
                            &normalized.permissions,
                            Arc::clone(&requested),
                            unresolved,
                            registry,
                            types,
                        )
                    });
            outcomes.push(outcome);
        }
        item_outcomes.push(outcomes);
    }
    Ok(item_outcomes)
}

fn collect_clause_outcomes(
    models: &TraceSpecModelStore,
    summaries: &SummaryStore<EffectUnit, super::domain::TraceSpecSummary>,
    registry: &EffectRegistry,
    types: &etas_types::TypeOutput,
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<BTreeMap<etas_hir::HirItemId, ClauseConformanceOutcomes>, EffectPipelineError> {
    let mut clause_outcomes = BTreeMap::new();
    for (item, conformances) in &models.referenced_by_item {
        let Some(summary) = summaries.get(EffectUnit::Item(*item)) else {
            continue;
        };
        if summary.incomplete {
            continue;
        }
        clause_outcomes.insert(
            *item,
            analyze_clause_groups(
                conformances,
                &summary.action_trace,
                &summary.requested_actions,
                registry,
                types,
                control,
            )?,
        );
    }
    Ok(clause_outcomes)
}

fn analyze_clause_groups(
    conformances: &super::model::TraceSpecConformances,
    trace: &crate::ActionTraceDomain,
    requested_actions: &crate::EffectRow,
    registry: &EffectRegistry,
    types: &etas_types::TypeOutput,
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<ClauseConformanceOutcomes, EffectPipelineError> {
    let requested = Arc::new(requested_actions.clone());
    let mut item_outcomes = Vec::with_capacity(conformances.len());
    for alternatives in conformances {
        let mut conformance_outcomes = Vec::with_capacity(alternatives.len());
        for clauses in alternatives {
            let mut clause_outcomes = Vec::with_capacity(clauses.len());
            for clause in clauses {
                if let Some(control) = control {
                    control
                        .checkpoint()
                        .map_err(EffectPipelineError::Interrupted)?;
                }
                let temporal =
                    |guard: &super::model::TraceSpecPattern,
                     target: &super::model::TraceSpecPattern| {
                        ClauseOutcome::Temporal(TemporalClauseOutcome {
                            source_clause: clause.source_clause(),
                            guard: guard.clone(),
                            target: target.clone(),
                            disposition: TemporalStaticDisposition::from_monitor(
                                require_before(trace, guard, target, registry, types),
                                requested_actions.open.is_some(),
                            ),
                        })
                    };
                let outcome = match clause {
                    TraceSpecClause::Deny { pattern, .. } => {
                        Some(ClauseOutcome::Denial(DenialClauseOutcome {
                            source_clause: clause.source_clause(),
                            pattern: pattern.clone(),
                            requested: Arc::clone(&requested),
                            unresolved_calls: trace.has_parameter_calls(),
                            disposition: analyze_denial(
                                pattern,
                                trace,
                                requested_actions,
                                registry,
                                types,
                            ),
                        }))
                    }
                    TraceSpecClause::RequireBefore { guard, target, .. } => {
                        Some(temporal(guard, target))
                    }
                    TraceSpecClause::RequireAfter {
                        target, obligation, ..
                    } => Some(temporal(target, obligation)),
                    _ => None,
                };
                clause_outcomes.push(outcome);
            }
            conformance_outcomes.push(clause_outcomes);
        }
        item_outcomes.push(conformance_outcomes);
    }
    Ok(item_outcomes)
}

fn collect_external_outcomes(
    models: &TraceSpecModelStore,
    external_summaries: &[crate::AnchoredExternalMetadata<crate::ExternalEffectSummaryMetadata>],
    effects: &mut EffectOutput,
    registry: &EffectRegistry,
    types: &etas_types::TypeOutput,
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<BTreeMap<super::model::ExternalTraceSpecItem, ExternalTraceOutcome>, EffectPipelineError>
{
    if models.external_referenced_by_item.is_empty() {
        return Ok(BTreeMap::new());
    }
    let max = etas_package_metadata::MAX_METADATA_GRAPH_NODES;
    if models.external_referenced_by_item.len() > max || external_summaries.len() > max {
        return Err(EffectPipelineError::AnalysisLimitExceeded {
            resource: "external trace spec summaries",
            limit: max,
        });
    }
    let mut by_owner = BTreeMap::<(u32, &[String]), Vec<_>>::new();
    for summary in external_summaries {
        if let Some(control) = control {
            control
                .checkpoint()
                .map_err(EffectPipelineError::Interrupted)?;
        }
        if super::validate::external_summary_matches_item(summary, &summary.metadata.item) {
            by_owner
                .entry((summary.package, &summary.metadata.item))
                .or_default()
                .push(summary);
        }
    }
    let mut output = BTreeMap::new();
    for (item, conformances) in &models.external_referenced_by_item {
        if let Some(control) = control {
            control
                .checkpoint()
                .map_err(EffectPipelineError::Interrupted)?;
        }
        let Some(matches) = by_owner.get(&(item.package, item.path.as_slice())) else {
            return Err(EffectPipelineError::InvalidExternalMetadata {
                package: item.package.to_string(),
                reason: format!(
                    "trace spec conformance for `{}` has no anchored solved effect summary",
                    item.path.join(".")
                ),
            });
        };
        let [summary] = matches.as_slice() else {
            return Err(EffectPipelineError::InvalidExternalMetadata {
                package: item.package.to_string(),
                reason: format!(
                    "trace spec conformance for `{}` has ambiguous solved effect summaries",
                    item.path.join(".")
                ),
            });
        };
        let Some(trace) = super::validate::trace_summary_from_external_metadata(
            summary,
            types,
            registry,
            effects,
            summary.span,
        ) else {
            output.insert(item.clone(), ExternalTraceOutcome::Invalid);
            continue;
        };
        let clauses = analyze_clause_groups(
            conformances,
            &trace.action_trace,
            &trace.requested_actions,
            registry,
            types,
            control,
        )?;
        let permissions = analyze_permission_groups(
            conformances,
            Arc::new(trace.allow_requested_actions.clone()),
            trace.action_trace.has_parameter_calls(),
            registry,
            types,
            control,
        )?;
        output.insert(
            item.clone(),
            ExternalTraceOutcome::Ready {
                span: summary.span,
                trace: Box::new(trace),
                clauses,
                permissions,
            },
        );
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ActionEvent, ActionEventSource, ActionTraceDomain, Effect, EffectRow, EffectSet,
        TraceSpecClauseFact, trace_spec::model::TraceSpecPattern,
    };
    use etas_core::{SourceId, Span, TextRange, TextSize};

    struct Stop;
    impl etas_utils::PipelineExecutionControl for Stop {
        fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
            Err(etas_utils::PipelineInterruption {
                kind: etas_utils::PipelineInterruptionKind::Cancelled,
                reason: "test cancellation".into(),
            })
        }
    }

    #[test]
    fn temporal_static_dispositions_preserve_unknowns_and_open_rows() {
        use TemporalMonitorResult as Result;
        use TemporalStaticDisposition as Disposition;
        use TemporalUncertainty as Uncertainty;
        assert_eq!(
            Disposition::from_monitor(Result::Satisfied, false),
            Disposition::Proven
        );
        assert_eq!(
            Disposition::from_monitor(Result::Satisfied, true),
            Disposition::Unsupported(Uncertainty::OpenRequestedActions)
        );
        assert_eq!(
            Disposition::from_monitor(Result::Violation, true),
            Disposition::Violated
        );
        assert_eq!(
            Disposition::from_monitor(Result::NeedsOutcomeEvidence, false),
            Disposition::Unsupported(Uncertainty::OutcomeEvidence)
        );
        assert_eq!(
            Disposition::from_monitor(Result::Unknown, false),
            Disposition::Unsupported(Uncertainty::OrderedActions)
        );
    }

    #[test]
    fn denial_analysis_preserves_definite_violations_and_unresolved_calls() {
        use crate::ParameterCallTarget;
        let registry = EffectRegistry::with_standard_effects();
        let types = etas_types::TypeOutput::default();
        let forbidden = Effect::Action(registry.action_by_name("Console.stdout_write").unwrap());
        let unrelated = Effect::Action(registry.action_by_name("Console.stdin_read_line").unwrap());
        let pattern = TraceSpecPattern {
            row: EffectRow::closed(EffectSet::one(forbidden.clone())),
            label: String::new(),
            arg_bounds: vec![],
        };
        let empty = EffectRow::empty();
        let trace = ActionTraceDomain::Empty;
        assert_eq!(
            analyze_denial(&pattern, &trace, &empty, &registry, &types),
            DenialStaticDisposition::Proven
        );
        assert_eq!(
            analyze_denial(
                &pattern,
                &trace,
                &EffectRow::closed(EffectSet::one(unrelated)),
                &registry,
                &types
            ),
            DenialStaticDisposition::Proven
        );
        let mut open = EffectRow::closed(EffectSet::one(forbidden));
        open.open = Some(crate::effect_var_id_from_name("E"));
        assert_eq!(
            analyze_denial(&pattern, &trace, &open, &registry, &types),
            DenialStaticDisposition::Violated
        );
        open.effects = EffectSet::default();
        assert_eq!(
            analyze_denial(&pattern, &trace, &open, &registry, &types),
            DenialStaticDisposition::Unsupported
        );
        let unresolved = ActionTraceDomain::ParameterCall {
            target: ParameterCallTarget::Interface(0),
            parameter: "f".into(),
            span: Span::new(SourceId(4), TextRange::new(TextSize(0), TextSize(1))),
        };
        assert_eq!(
            analyze_denial(&pattern, &unresolved, &empty, &registry, &types),
            DenialStaticDisposition::Unsupported
        );
        let memory_read = registry.action_by_name("Memory.read").unwrap();
        let applied = |arg| {
            Effect::AppliedAction(crate::ActionInstanceRef {
                action: memory_read.clone(),
                args: vec![arg],
            })
        };
        let narrow = TraceSpecPattern {
            row: EffectRow::closed(EffectSet::one(applied(etas_types::EffectArgRef::Path(
                vec!["ProjectMemory".into()],
            )))),
            label: String::new(),
            arg_bounds: vec![],
        };
        let unknown =
            EffectRow::closed(EffectSet::one(applied(etas_types::EffectArgRef::Wildcard)));
        assert_eq!(
            analyze_denial(&narrow, &trace, &unknown, &registry, &types),
            DenialStaticDisposition::Unsupported,
        );
    }

    #[test]
    fn analysis_emits_denial_outcomes_from_solved_summary() {
        let item = etas_hir::HirItemId(7);
        let span = Span::new(SourceId(4), TextRange::new(TextSize(0), TextSize(1)));
        let registry = EffectRegistry::with_standard_effects();
        let types = etas_types::TypeOutput::default();
        let action = Effect::Action(registry.action_by_name("Console.stdout_write").unwrap());
        let row = EffectRow::closed(EffectSet::one(action));
        let pattern = TraceSpecPattern {
            row: row.clone(),
            label: String::new(),
            arg_bounds: vec![],
        };
        let clause = TraceSpecClause::Deny {
            source_path: Some(vec![0]),
            source_clause: None,
            pattern: pattern.clone(),
            fact: TraceSpecClauseFact::Deny {
                pattern: row.clone(),
                label: String::new(),
            },
            span,
        };
        let mut models = TraceSpecModelStore::default();
        models
            .referenced_by_item
            .insert(item, vec![vec![vec![clause]]]);
        let mut summary = super::super::domain::TraceSpecSummary::for_unit(EffectUnit::Item(item));
        summary.requested_actions = row.clone();
        let mut summaries = SummaryStore::new();
        summaries.insert(EffectUnit::Item(item), summary);
        let outcomes =
            collect_clause_outcomes(&models, &summaries, &registry, &types, None).unwrap();
        assert_eq!(
            outcomes[&item][0][0][0],
            Some(ClauseOutcome::Denial(DenialClauseOutcome {
                source_clause: None,
                pattern,
                requested: Arc::new(row),
                unresolved_calls: false,
                disposition: DenialStaticDisposition::Violated,
            }))
        );
    }

    #[test]
    fn permission_analysis_keeps_alternatives_separate() {
        let item = etas_hir::HirItemId(9);
        let span = Span::new(SourceId(4), TextRange::new(TextSize(0), TextSize(1)));
        let registry = EffectRegistry::with_standard_effects();
        let types = etas_types::TypeOutput::default();
        let action = |name| Effect::Action(registry.action_by_name(name).unwrap());
        let output = action("Console.stdout_write");
        let input = action("Console.stdin_read_line");
        let allow = |effect| {
            let row = EffectRow::closed(EffectSet::one(effect));
            TraceSpecClause::Allow {
                source_path: Some(vec![0]),
                source_clause: None,
                pattern: TraceSpecPattern {
                    row: row.clone(),
                    label: String::new(),
                    arg_bounds: vec![],
                },
                fact: TraceSpecClauseFact::Allow {
                    pattern: row,
                    label: String::new(),
                },
                span,
            }
        };
        let mut models = TraceSpecModelStore::default();
        models.referenced_by_item.insert(
            item,
            vec![vec![vec![allow(output.clone())], vec![allow(input)]]],
        );
        let mut summary = super::super::domain::TraceSpecSummary::for_unit(EffectUnit::Item(item));
        summary.requested_actions = EffectRow::closed(EffectSet::one(output.clone()));
        let mut summaries = SummaryStore::new();
        summaries.insert(EffectUnit::Item(item), summary);
        let mut effects = EffectOutput::default();
        effects
            .facts
            .item_effects
            .insert(item, crate::EffectSummary::local());
        let outcomes =
            collect_permission_outcomes(&models, &summaries, &effects, &registry, &types, None)
                .unwrap();
        assert!(outcomes[&item][0][0].as_ref().unwrap().uncovered.is_empty());
        assert_eq!(
            outcomes[&item][0][1].as_ref().unwrap().uncovered,
            vec![(output.clone(), false)]
        );
        let mut checked = TraceSpecAnalysisOutput {
            summaries: SummaryStore::new(),
            source_conformances: BTreeMap::from([(
                0,
                CheckedTraceSourceConformance {
                    item,
                    span,
                    source_owner: None,
                    source_instance: None,
                    clause_ids: vec![vec![None], vec![None]],
                    analyzed_alternatives: Arc::new(models.referenced_by_item[&item][0].clone()),
                    conformance_position: 0,
                    clause_kinds: vec![
                        vec![SourceClauseKind::Allow {
                            permission_index: 0,
                            origin_index: 0,
                        }],
                        vec![SourceClauseKind::Allow {
                            permission_index: 0,
                            origin_index: 0,
                        }],
                    ],
                },
            )]),
            clause_outcomes: BTreeMap::from([(item, vec![vec![vec![None], vec![None]]])]),
            permission_outcomes: outcomes.clone(),
            external_outcomes: BTreeMap::new(),
        };
        assert_eq!(
            checked.checked_conformance_disposition(0, None),
            Ok(CheckedTraceStaticDisposition::Proven)
        );
        let classified =
            super::super::collect_trace_static_dispositions(&checked, None, None).unwrap();
        assert_eq!(
            classified.conformance(0, item, &checked, None),
            checked.checked_conformance_disposition(0, None)
        );
        assert_eq!(
            classified
                .verified_conformance(0, item, &checked, None)
                .unwrap()
                .disposition(),
            CheckedTraceStaticDisposition::Proven,
        );
        let lowering = classified
            .analyzed_lowering_source(0, item, &checked, None)
            .unwrap();
        assert_eq!(lowering, &models.referenced_by_item[&item][0]);
        let patterns = lowering
            .iter()
            .flatten()
            .map(|clause| {
                let TraceSpecClause::Allow { pattern, .. } = clause else {
                    panic!("expected allow clause")
                };
                super::super::TracePatternProjection {
                    source: pattern,
                    pattern: super::super::lower::tests::pattern("test"),
                }
            })
            .collect::<Vec<_>>();
        let input = |item| super::super::AnalyzedTraceLoweringInput {
            analysis: &checked,
            dispositions: &classified,
            type_fact_index: 0,
            item,
            limits: super::super::lower::tests::limits(),
            control: None,
        };
        assert!(super::super::lower_analyzed_trace_rules(input(item), patterns.clone()).is_ok());
        assert!(matches!(
            super::super::lower_analyzed_trace_rules(input(etas_hir::HirItemId(10)), patterns),
            Err(super::super::TraceRulesLoweringError::Analysis(_))
        ));
        let mut changed_model = models.clone();
        changed_model.referenced_by_item.get_mut(&item).unwrap()[0].pop();
        assert_ne!(lowering, &changed_model.referenced_by_item[&item][0]);

        let mut changed_analysis = checked.clone();
        let source = changed_analysis.source_conformances.get_mut(&0).unwrap();
        Arc::make_mut(&mut source.analyzed_alternatives).pop();
        assert!(
            classified
                .analyzed_lowering_source(0, item, &changed_analysis, None)
                .is_err()
        );
        let reclassified =
            super::super::collect_trace_static_dispositions(&changed_analysis, None, None).unwrap();
        assert!(
            reclassified
                .analyzed_lowering_source(0, item, &changed_analysis, None)
                .is_err()
        );
        let mut multiple_clauses = checked.clone();
        let source = multiple_clauses.source_conformances.get_mut(&0).unwrap();
        source.clause_ids[0].push(None);
        source.clause_kinds[0].push(SourceClauseKind::Allow {
            permission_index: 0,
            origin_index: 0,
        });
        multiple_clauses.clause_outcomes.get_mut(&item).unwrap()[0][0].push(None);
        super::super::static_dispositions::assert_rejects_incomplete_classification(
            &multiple_clauses,
            0,
            item,
        );
        let classified_source = classified.checked_conformance(0, item, None).unwrap();
        assert_eq!(classified_source.source(), &checked.source_conformances[&0]);
        assert_eq!(classified_source.alternatives().len(), 2);
        assert_eq!(
            classified_source.alternatives()[0].permission(),
            CheckedTracePermissionDisposition::Proven
        );
        assert_eq!(
            classified_source.alternatives()[1].permission(),
            CheckedTracePermissionDisposition::Violated
        );
        let evidence = classified_source.alternatives()[1].permission_evidence();
        assert_eq!(
            evidence.requested(),
            &EffectRow::closed(EffectSet::one(output.clone()))
        );
        assert!(!evidence.unresolved());
        assert!(!evidence.unresolved_calls());
        assert_eq!(evidence.uncovered().len(), 1);
        assert_eq!(evidence.uncovered()[0].action(), &output);
        assert_eq!(
            evidence.uncovered()[0].coverage(),
            CheckedPermissionCoverage::Uncovered
        );
        assert_eq!(
            classified_source.alternatives()[0].clauses()[0],
            checked.checked_clause_fact(0, 0, 0).unwrap()
        );
        let previous = checked.permission_outcomes[&item][0][0]
            .as_ref()
            .unwrap()
            .requested
            .clone();
        checked.permission_outcomes.get_mut(&item).unwrap()[0][0]
            .as_mut()
            .unwrap()
            .requested = Arc::new(EffectRow::empty());
        assert!(classified.conformance(0, item, &checked, None).is_err());
        checked.permission_outcomes.get_mut(&item).unwrap()[0][0]
            .as_mut()
            .unwrap()
            .requested = previous;
        assert!(
            classified
                .conformance(0, etas_hir::HirItemId(10), &checked, None)
                .is_err()
        );
        let unreachable = std::collections::BTreeSet::new();
        let skipped =
            super::super::collect_trace_static_dispositions(&checked, Some(&unreachable), None)
                .unwrap();
        assert!(skipped.conformance(0, item, &checked, None).is_err());
        struct StopAfterOne(std::sync::atomic::AtomicUsize);
        impl etas_utils::PipelineExecutionControl for StopAfterOne {
            fn checkpoint(&self) -> Result<(), etas_utils::PipelineInterruption> {
                if self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0 {
                    Ok(())
                } else {
                    Err(etas_utils::PipelineInterruption {
                        kind: etas_utils::PipelineInterruptionKind::Cancelled,
                        reason: "test cancellation".into(),
                    })
                }
            }
        }
        assert!(matches!(
            checked.checked_conformance_disposition(
                0,
                Some(&StopAfterOne(std::sync::atomic::AtomicUsize::new(0)))
            ),
            Err(EffectPipelineError::Interrupted(_))
        ));
        assert!(matches!(
            classified.verified_conformance(
                0,
                item,
                &checked,
                Some(&StopAfterOne(std::sync::atomic::AtomicUsize::new(0))),
            ),
            Err(EffectPipelineError::Interrupted(_))
        ));
        assert!(matches!(
            super::super::collect_trace_static_dispositions(
                &checked,
                None,
                Some(&StopAfterOne(std::sync::atomic::AtomicUsize::new(0))),
            ),
            Err(EffectPipelineError::Interrupted(_))
        ));
        checked.permission_outcomes.get_mut(&item).unwrap()[0][0]
            .as_mut()
            .unwrap()
            .unresolved = true;
        checked.permission_outcomes.get_mut(&item).unwrap()[0][0]
            .as_mut()
            .unwrap()
            .unresolved_calls = true;
        assert!(classified.conformance(0, item, &checked, None).is_err());
        let classified =
            super::super::collect_trace_static_dispositions(&checked, None, None).unwrap();
        assert_eq!(
            classified.conformance(0, item, &checked, None),
            Ok(CheckedTraceStaticDisposition::Unsupported)
        );
        assert!(
            classified
                .checked_conformance(0, item, None)
                .unwrap()
                .alternatives()[0]
                .permission_evidence()
                .unresolved_calls()
        );
        assert_eq!(
            classified.conformance(0, item, &checked, None),
            checked.checked_conformance_disposition(0, None)
        );
        checked.permission_outcomes.get_mut(&item).unwrap()[0][0]
            .as_mut()
            .unwrap()
            .unresolved = false;
        checked.permission_outcomes.get_mut(&item).unwrap()[0][0]
            .as_mut()
            .unwrap()
            .unresolved_calls = false;
        let violated = checked.permission_outcomes[&item][0][1].clone();
        checked.permission_outcomes.get_mut(&item).unwrap()[0][0] = violated;
        assert_eq!(
            checked.checked_conformance_disposition(0, None),
            Ok(CheckedTraceStaticDisposition::Violated)
        );
        let classified =
            super::super::collect_trace_static_dispositions(&checked, None, None).unwrap();
        assert_eq!(
            classified.conformance(0, item, &checked, None),
            checked.checked_conformance_disposition(0, None)
        );
        checked.permission_outcomes.get_mut(&item).unwrap()[0][1] = None;
        assert!(checked.checked_conformance_disposition(0, None).is_err());
        let incomplete =
            super::super::collect_trace_static_dispositions(&checked, None, None).unwrap();
        assert!(incomplete.conformance(0, item, &checked, None).is_err());
        effects
            .facts
            .item_effects
            .get_mut(&item)
            .unwrap()
            .default_actions = EffectRow::closed(EffectSet::one(output));
        let handled =
            collect_permission_outcomes(&models, &summaries, &effects, &registry, &types, None)
                .unwrap();
        assert!(
            handled[&item][0]
                .iter()
                .all(|outcome| outcome.as_ref().unwrap().uncovered.is_empty())
        );
        assert!(matches!(
            collect_permission_outcomes(
                &models,
                &summaries,
                &effects,
                &registry,
                &types,
                Some(&Stop)
            ),
            Err(EffectPipelineError::Interrupted(_))
        ));
    }

    #[test]
    fn external_analysis_and_validation_share_checked_results_and_reject_bad_metadata() {
        use crate::{
            AnchoredExternalMetadata, ExternalActionTraceMetadata, ExternalEffectMetadata,
            ExternalEffectRowMetadata, ExternalEffectSummaryMetadata,
        };
        let span = Span::new(SourceId(9), TextRange::new(TextSize(0), TextSize(4)));
        let registry = EffectRegistry::with_standard_effects();
        let types = etas_types::TypeOutput::default();
        let action = Effect::Action(registry.action_by_name("Console.stdout_write").unwrap());
        let row = EffectRow::closed(EffectSet::one(action));
        let clause = TraceSpecClause::Deny {
            source_path: None,
            source_clause: None,
            pattern: TraceSpecPattern {
                row: row.clone(),
                label: String::new(),
                arg_bounds: vec![],
            },
            fact: TraceSpecClauseFact::Deny {
                pattern: row,
                label: String::new(),
            },
            span,
        };
        let item = super::super::model::ExternalTraceSpecItem {
            package: 7,
            path: vec!["dep".into(), "run".into()],
        };
        let mut models = TraceSpecModelStore::default();
        models
            .external_referenced_by_item
            .insert(item.clone(), vec![vec![vec![clause]]]);
        let action = ExternalEffectMetadata {
            path: vec!["Console".into(), "stdout_write".into()],
            args: vec![],
        };
        let summary = AnchoredExternalMetadata {
            package: 7,
            span,
            metadata: ExternalEffectSummaryMetadata {
                item: item.path.clone(),
                requested_actions: ExternalEffectRowMetadata {
                    effects: vec![action.clone()],
                    tail: None,
                },
                action_trace: ExternalActionTraceMetadata::Event {
                    action,
                    source: ActionEventSource::ExternalMetadata,
                },
                ..Default::default()
            },
        };
        let mut effects = EffectOutput::default();
        let outcomes = collect_external_outcomes(
            &models,
            std::slice::from_ref(&summary),
            &mut effects,
            &registry,
            &types,
            None,
        )
        .unwrap();
        assert!(
            matches!(outcomes.get(&item), Some(ExternalTraceOutcome::Ready { clauses, .. })
            if matches!(&clauses[0][0][0], Some(ClauseOutcome::Denial(DenialClauseOutcome { disposition: DenialStaticDisposition::Violated, .. }))))
        );
        let analysis = TraceSpecAnalysisOutput {
            summaries: SummaryStore::new(),
            source_conformances: Default::default(),
            clause_outcomes: Default::default(),
            permission_outcomes: Default::default(),
            external_outcomes: outcomes,
        };
        super::super::validate::validate_external_item_policies(
            &types,
            &registry,
            &mut effects,
            &models,
            &analysis,
        )
        .unwrap();
        assert!(effects.diagnostics.iter().any(|diagnostic| diagnostic.code
            == etas_core::DiagnosticCode::Effect(
                etas_core::EffectDiagnosticCode::TraceSpecDenied
            )
            && diagnostic.primary.span == span));
        let mut malformed = summary.clone();
        malformed.metadata.action_trace = ExternalActionTraceMetadata::Empty;
        let mut invalid_effects = EffectOutput::default();
        let invalid = collect_external_outcomes(
            &models,
            &[malformed],
            &mut invalid_effects,
            &registry,
            &types,
            None,
        )
        .unwrap();
        assert!(matches!(
            invalid.get(&item),
            Some(ExternalTraceOutcome::Invalid)
        ));
        assert!(
            invalid_effects
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code
                    == etas_core::DiagnosticCode::Effect(
                        etas_core::EffectDiagnosticCode::IncompleteEffectFacts
                    )
                    && diagnostic.primary.span == span)
        );
        assert!(matches!(
            collect_external_outcomes(
                &models,
                &[],
                &mut EffectOutput::default(),
                &registry,
                &types,
                None
            ),
            Err(EffectPipelineError::InvalidExternalMetadata { .. })
        ));
        let mut other_package = summary.clone();
        other_package.package = 8;
        assert!(matches!(
            collect_external_outcomes(
                &models,
                std::slice::from_ref(&other_package),
                &mut EffectOutput::default(),
                &registry,
                &types,
                None,
            ),
            Err(EffectPipelineError::InvalidExternalMetadata { .. })
        ));
        let mut wrong_root = summary.clone();
        wrong_root.metadata.import_root = Some("another".into());
        assert!(matches!(
            collect_external_outcomes(
                &models,
                std::slice::from_ref(&wrong_root),
                &mut EffectOutput::default(),
                &registry,
                &types,
                None,
            ),
            Err(EffectPipelineError::InvalidExternalMetadata { .. })
        ));
        assert!(
            collect_external_outcomes(
                &models,
                &[wrong_root, summary.clone()],
                &mut EffectOutput::default(),
                &registry,
                &types,
                None,
            )
            .is_ok()
        );
        let isolated = collect_external_outcomes(
            &models,
            &[other_package, summary.clone()],
            &mut EffectOutput::default(),
            &registry,
            &types,
            None,
        )
        .unwrap();
        assert!(matches!(
            isolated.get(&item),
            Some(ExternalTraceOutcome::Ready { .. })
        ));
        assert!(matches!(
            collect_external_outcomes(
                &models,
                &[summary.clone(), summary],
                &mut EffectOutput::default(),
                &registry,
                &types,
                None
            ),
            Err(EffectPipelineError::InvalidExternalMetadata { .. })
        ));
    }

    #[test]
    fn clause_outcomes_preserve_clause_positions_and_require_complete_summaries() {
        let item = etas_hir::HirItemId(3);
        let span = Span::new(SourceId(4), TextRange::new(TextSize(0), TextSize(1)));
        let registry = EffectRegistry::with_standard_effects();
        let types = etas_types::TypeOutput::default();
        let action = |name| Effect::Action(registry.action_by_name(name).unwrap());
        let guard_effect = action("Console.stdin_read_line");
        let target_effect = action("Console.stdout_write");
        let pattern = |effect| TraceSpecPattern {
            row: EffectRow::closed(EffectSet::one(effect)),
            label: String::new(),
            arg_bounds: vec![],
        };
        let guard = pattern(guard_effect.clone());
        let target = pattern(target_effect.clone());
        let allow = TraceSpecClause::Allow {
            source_path: None,
            source_clause: None,
            pattern: guard.clone(),
            fact: TraceSpecClauseFact::Allow {
                pattern: guard.row.clone(),
                label: String::new(),
            },
            span,
        };
        let before = TraceSpecClause::RequireBefore {
            source_path: None,
            source_clause: None,
            guard: guard.clone(),
            target: target.clone(),
            fact: TraceSpecClauseFact::RequireBefore {
                guard: guard.row.clone(),
                guard_label: String::new(),
                target: target.row.clone(),
                target_label: String::new(),
            },
            span,
        };
        let after = TraceSpecClause::RequireAfter {
            source_path: None,
            source_clause: None,
            target: guard.clone(),
            obligation: target.clone(),
            fact: TraceSpecClauseFact::RequireAfter {
                target: guard.row.clone(),
                target_label: String::new(),
                obligation: target.row.clone(),
                obligation_label: String::new(),
            },
            span,
        };
        let mut models = TraceSpecModelStore::default();
        models
            .referenced_by_item
            .insert(item, vec![vec![vec![allow, before, after]]]);
        let event = |action| {
            ActionTraceDomain::Event(ActionEvent {
                action,
                span,
                source: ActionEventSource::Perform,
            })
        };
        let mut summary = super::super::domain::TraceSpecSummary::for_unit(EffectUnit::Item(item));
        summary.action_trace =
            ActionTraceDomain::Seq(vec![event(guard_effect), event(target_effect)]);
        let mut summaries = SummaryStore::new();
        summaries.insert(EffectUnit::Item(item), summary.clone());
        let outcomes =
            collect_clause_outcomes(&models, &summaries, &registry, &types, None).unwrap();
        assert_eq!(
            outcomes[&item],
            vec![vec![vec![
                None,
                Some(ClauseOutcome::Temporal(TemporalClauseOutcome {
                    source_clause: None,
                    guard: guard.clone(),
                    target: target.clone(),
                    disposition: TemporalStaticDisposition::Unsupported(
                        TemporalUncertainty::OutcomeEvidence
                    )
                })),
                Some(ClauseOutcome::Temporal(TemporalClauseOutcome {
                    source_clause: None,
                    guard,
                    target,
                    disposition: TemporalStaticDisposition::Unsupported(
                        TemporalUncertainty::OutcomeEvidence
                    )
                }))
            ]]]
        );
        assert!(matches!(
            collect_clause_outcomes(&models, &summaries, &registry, &types, Some(&Stop)),
            Err(EffectPipelineError::Interrupted(
                etas_utils::PipelineInterruption {
                    kind: etas_utils::PipelineInterruptionKind::Cancelled,
                    ..
                }
            ))
        ));
        summary.action_trace = ActionTraceDomain::Empty;
        summary.requested_actions.open = Some(crate::effect::effect_var_id_from_name("E"));
        summaries.insert(EffectUnit::Item(item), summary.clone());
        let open_outcomes =
            collect_clause_outcomes(&models, &summaries, &registry, &types, None).unwrap();
        assert!(matches!(
            &open_outcomes[&item][0][0][1],
            Some(ClauseOutcome::Temporal(TemporalClauseOutcome {
                disposition: TemporalStaticDisposition::Unsupported(
                    TemporalUncertainty::OpenRequestedActions
                ),
                ..
            }))
        ));
        summary.incomplete = true;
        summaries.insert(EffectUnit::Item(item), summary);
        assert!(
            !collect_clause_outcomes(&models, &summaries, &registry, &types, None)
                .unwrap()
                .contains_key(&item)
        );
        assert!(
            !collect_clause_outcomes(&models, &SummaryStore::new(), &registry, &types, None)
                .unwrap()
                .contains_key(&item)
        );
    }
}
