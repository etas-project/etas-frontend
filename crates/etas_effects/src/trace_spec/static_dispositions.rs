//! Typed static trace conclusions. These are not residual plans or evidence of
//! runtime installation.

use std::collections::{BTreeMap, BTreeSet};

use etas_hir::HirItemId;

use super::analysis::{
    CheckedPermissionCoverage, CheckedTraceClauseFact, CheckedTracePermissionDisposition,
    CheckedTracePermissionFact, CheckedTraceSourceConformance, CheckedTraceStaticDisposition,
    TraceSpecAnalysisOutput,
};
use crate::EffectPipelineError;

pub struct CheckedTraceStaticAlternative {
    permission: CheckedTraceStaticPermission,
    clauses: Vec<CheckedTraceClauseFact>,
}

/// Exact analysis inputs retained for lowering. An unknown coverage is not
/// itself evidence that an executable predicate or observation site exists.
pub struct CheckedTraceStaticPermission {
    disposition: CheckedTracePermissionDisposition,
    requested: crate::EffectRow,
    unresolved: bool,
    unresolved_calls: bool,
    uncovered: Vec<CheckedTraceStaticUncoveredAction>,
}

#[derive(PartialEq, Eq)]
pub struct CheckedTraceStaticUncoveredAction {
    action: crate::Effect,
    coverage: CheckedPermissionCoverage,
}

pub struct CheckedTraceStaticConformance {
    source: CheckedTraceSourceConformance,
    alternatives: Vec<CheckedTraceStaticAlternative>,
    disposition: CheckedTraceStaticDisposition,
}

impl CheckedTraceStaticAlternative {
    pub fn permission(&self) -> CheckedTracePermissionDisposition {
        self.permission.disposition
    }

    pub fn permission_evidence(&self) -> &CheckedTraceStaticPermission {
        &self.permission
    }

    pub fn clauses(&self) -> &[CheckedTraceClauseFact] {
        &self.clauses
    }
}

impl CheckedTraceStaticPermission {
    pub fn requested(&self) -> &crate::EffectRow {
        &self.requested
    }

    pub fn unresolved(&self) -> bool {
        self.unresolved
    }

    pub fn unresolved_calls(&self) -> bool {
        self.unresolved_calls
    }

    pub fn uncovered(&self) -> &[CheckedTraceStaticUncoveredAction] {
        &self.uncovered
    }

    fn matches(&self, fact: &CheckedTracePermissionFact<'_>) -> bool {
        self.disposition == fact.disposition()
            && self.requested == *fact.requested
            && self.unresolved == fact.unresolved
            && self.unresolved_calls == fact.unresolved_calls
            && self.uncovered.len() == fact.uncovered().len()
            && self
                .uncovered
                .iter()
                .zip(fact.uncovered())
                .all(|(left, right)| {
                    left.action == *right.action && left.coverage == right.coverage
                })
    }
}

impl CheckedTraceStaticUncoveredAction {
    pub fn action(&self) -> &crate::Effect {
        &self.action
    }

    pub fn coverage(&self) -> CheckedPermissionCoverage {
        self.coverage
    }
}

impl CheckedTraceStaticConformance {
    pub fn source(&self) -> &CheckedTraceSourceConformance {
        &self.source
    }

    pub fn alternatives(&self) -> &[CheckedTraceStaticAlternative] {
        &self.alternatives
    }

    pub fn disposition(&self) -> CheckedTraceStaticDisposition {
        self.disposition
    }
}

#[derive(Default)]
pub struct TraceStaticDispositions {
    // Invalid per-item analysis is retained for the validator to report at the
    // original source anchor, rather than turning it into an empty result.
    conformances: BTreeMap<usize, Result<CheckedTraceStaticConformance, EffectPipelineError>>,
}

#[cfg(test)]
pub(super) fn assert_rejects_incomplete_classification(
    analysis: &TraceSpecAnalysisOutput,
    index: usize,
    item: HirItemId,
) {
    let collect = || collect_trace_static_dispositions(analysis, None, None).unwrap();
    let mut missing_alternative = collect();
    let record = missing_alternative
        .conformances
        .get_mut(&index)
        .unwrap()
        .as_mut()
        .unwrap();
    assert!(record.alternatives.len() > 1);
    record.alternatives.pop();
    assert!(
        missing_alternative
            .checked_conformance(index, item, None)
            .is_err()
    );
    assert!(
        missing_alternative
            .conformance(index, item, analysis, None)
            .is_err()
    );
    assert!(
        missing_alternative
            .verified_conformance(index, item, analysis, None)
            .is_err()
    );

    let mut missing_clause = collect();
    let record = missing_clause
        .conformances
        .get_mut(&index)
        .unwrap()
        .as_mut()
        .unwrap();
    assert!(record.alternatives[0].clauses.len() > 1);
    record.alternatives[0].clauses.pop();
    assert!(
        missing_clause
            .checked_conformance(index, item, None)
            .is_err()
    );
    assert!(
        missing_clause
            .conformance(index, item, analysis, None)
            .is_err()
    );
    assert!(
        missing_clause
            .verified_conformance(index, item, analysis, None)
            .is_err()
    );

    let mut false_disposition = collect();
    let record = false_disposition
        .conformances
        .get_mut(&index)
        .unwrap()
        .as_mut()
        .unwrap();
    record.disposition = match record.disposition {
        CheckedTraceStaticDisposition::Proven => CheckedTraceStaticDisposition::Violated,
        _ => CheckedTraceStaticDisposition::Proven,
    };
    assert!(
        false_disposition
            .conformance(index, item, analysis, None)
            .is_err()
    );
    assert!(
        false_disposition
            .verified_conformance(index, item, analysis, None)
            .is_err()
    );
}

impl TraceStaticDispositions {
    /// Classified checked type-fact indices, including invalid records that
    /// validation reports at their source anchor.
    pub fn indices(&self) -> impl ExactSizeIterator<Item = usize> + '_ {
        self.conformances.keys().copied()
    }

    pub(crate) fn checked_conformance(
        &self,
        index: usize,
        item: HirItemId,
        control: Option<&dyn etas_utils::PipelineExecutionControl>,
    ) -> Result<&CheckedTraceStaticConformance, EffectPipelineError> {
        let invalid = || EffectPipelineError::InvalidTraceSpecSourceSites {
            reason: "trace static dispositions do not match their checked source".into(),
        };
        let conformance = self.conformances.get(&index).ok_or_else(invalid)?;
        let conformance = conformance.as_ref().map_err(Clone::clone)?;
        if conformance.source.item != item
            || conformance.alternatives.is_empty()
            || conformance.alternatives.len() != conformance.source.clause_ids.len()
        {
            return Err(invalid());
        }
        for (alternative, clauses) in conformance
            .alternatives
            .iter()
            .zip(&conformance.source.clause_ids)
        {
            if let Some(control) = control {
                control
                    .checkpoint()
                    .map_err(EffectPipelineError::Interrupted)?;
            }
            if alternative.clauses.is_empty() || alternative.clauses.len() != clauses.len() {
                return Err(invalid());
            }
        }
        Ok(conformance)
    }

    pub(crate) fn conformance(
        &self,
        index: usize,
        item: HirItemId,
        analysis: &TraceSpecAnalysisOutput,
        control: Option<&dyn etas_utils::PipelineExecutionControl>,
    ) -> Result<CheckedTraceStaticDisposition, EffectPipelineError> {
        Ok(self
            .verified_conformance(index, item, analysis, control)?
            .disposition)
    }

    /// Returns the exact classified source facts only after revalidating every
    /// branch, clause, permission and the combined conclusion against analysis.
    /// This is not residual plan coverage or runtime installation evidence.
    pub fn verified_conformance(
        &self,
        index: usize,
        item: HirItemId,
        analysis: &TraceSpecAnalysisOutput,
        control: Option<&dyn etas_utils::PipelineExecutionControl>,
    ) -> Result<&CheckedTraceStaticConformance, EffectPipelineError> {
        let invalid = || EffectPipelineError::InvalidTraceSpecSourceSites {
            reason: "trace static dispositions do not match their checked source".into(),
        };
        let conformance = self.checked_conformance(index, item, control)?;
        if analysis.source_conformance(index) != Some(&conformance.source) {
            return Err(invalid());
        }
        for (alternative_index, alternative) in conformance.alternatives.iter().enumerate() {
            if let Some(control) = control {
                control
                    .checkpoint()
                    .map_err(EffectPipelineError::Interrupted)?;
            }
            if !alternative
                .permission
                .matches(&analysis.checked_permission_fact(index, alternative_index)?)
            {
                return Err(invalid());
            }
            for (clause_index, clause) in alternative.clauses.iter().enumerate() {
                if let Some(control) = control {
                    control
                        .checkpoint()
                        .map_err(EffectPipelineError::Interrupted)?;
                }
                if *clause
                    != analysis.checked_clause_fact(index, alternative_index, clause_index)?
                {
                    return Err(invalid());
                }
            }
        }
        if conformance.disposition != analysis.checked_conformance_disposition(index, control)? {
            return Err(invalid());
        }
        Ok(conformance)
    }

    /// Returns the exact rules analyzed for this conformance, after rechecking
    /// its classification. This is an input to lowering, not plan coverage or
    /// proof that any projected action identity is correct.
    pub fn analyzed_lowering_source(
        &self,
        index: usize,
        item: HirItemId,
        analysis: &TraceSpecAnalysisOutput,
        control: Option<&dyn etas_utils::PipelineExecutionControl>,
    ) -> Result<&super::TraceSpecClauseAlternatives, EffectPipelineError> {
        let checked = self.verified_conformance(index, item, analysis, control)?;
        let source = &checked.source;
        let rules = source.analyzed_alternatives.as_ref();
        let invalid = || EffectPipelineError::InvalidTraceSpecSourceSites {
            reason: "analyzed trace rules do not match their checked source".into(),
        };
        if rules.len() != source.clause_ids.len() {
            return Err(invalid());
        }
        for (group, ids) in rules.iter().zip(&source.clause_ids) {
            if group.len() != ids.len() {
                return Err(invalid());
            }
            for (clause, id) in group.iter().zip(ids) {
                if let Some(control) = control {
                    control
                        .checkpoint()
                        .map_err(EffectPipelineError::Interrupted)?;
                }
                if clause.source_clause() != *id {
                    return Err(invalid());
                }
            }
        }
        Ok(rules)
    }
}

pub fn collect_trace_static_dispositions(
    analysis: &TraceSpecAnalysisOutput,
    reachable: Option<&BTreeSet<HirItemId>>,
    control: Option<&dyn etas_utils::PipelineExecutionControl>,
) -> Result<TraceStaticDispositions, EffectPipelineError> {
    let limit = etas_package_metadata::MAX_METADATA_GRAPH_NODES;
    let mut remaining = limit;
    let mut conformances = BTreeMap::new();
    for (&index, source) in &analysis.source_conformances {
        if reachable.is_some_and(|items| !items.contains(&source.item)) {
            continue;
        }
        if let Some(control) = control {
            control
                .checkpoint()
                .map_err(EffectPipelineError::Interrupted)?;
        }
        remaining = remaining
            .checked_sub(1)
            .ok_or(EffectPipelineError::AnalysisLimitExceeded {
                resource: "trace static dispositions",
                limit,
            })?;
        let value = (|| {
            remaining = remaining.checked_sub(source.clause_ids.len()).ok_or(
                EffectPipelineError::AnalysisLimitExceeded {
                    resource: "trace static dispositions",
                    limit,
                },
            )?;
            let mut alternatives = Vec::with_capacity(source.clause_ids.len());
            for (alternative_index, clauses) in source.clause_ids.iter().enumerate() {
                remaining = remaining.checked_sub(clauses.len()).ok_or(
                    EffectPipelineError::AnalysisLimitExceeded {
                        resource: "trace static dispositions",
                        limit,
                    },
                )?;
                let permission = analysis.checked_permission_fact(index, alternative_index)?;
                remaining = remaining
                    .checked_sub(permission.requested.effects.len())
                    .and_then(|value| value.checked_sub(permission.uncovered().len()))
                    .ok_or(EffectPipelineError::AnalysisLimitExceeded {
                        resource: "trace static dispositions",
                        limit,
                    })?;
                let permission = CheckedTraceStaticPermission {
                    disposition: permission.disposition(),
                    requested: permission.requested.clone(),
                    unresolved: permission.unresolved,
                    unresolved_calls: permission.unresolved_calls,
                    uncovered: permission
                        .uncovered()
                        .map(|entry| CheckedTraceStaticUncoveredAction {
                            action: entry.action.clone(),
                            coverage: entry.coverage,
                        })
                        .collect(),
                };
                let mut facts = Vec::with_capacity(clauses.len());
                for clause_index in 0..clauses.len() {
                    if let Some(control) = control {
                        control
                            .checkpoint()
                            .map_err(EffectPipelineError::Interrupted)?;
                    }
                    facts.push(analysis.checked_clause_fact(
                        index,
                        alternative_index,
                        clause_index,
                    )?);
                }
                alternatives.push(CheckedTraceStaticAlternative {
                    permission,
                    clauses: facts,
                });
            }
            Ok(CheckedTraceStaticConformance {
                source: source.clone(),
                alternatives,
                disposition: analysis.checked_conformance_disposition(index, control)?,
            })
        })();
        match value {
            Err(error @ EffectPipelineError::InvalidTraceSpecSourceSites { .. }) => {
                conformances.insert(index, Err(error));
            }
            Err(error) => return Err(error),
            Ok(value) => {
                conformances.insert(index, Ok(value));
            }
        }
    }
    Ok(TraceStaticDispositions { conformances })
}
