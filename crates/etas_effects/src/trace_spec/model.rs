use std::collections::BTreeMap;

use etas_core::Span;
use etas_hir::HirItemId;
use etas_types::Assignable;

use crate::{
    ActionEvent, ActionRef, Effect, EffectCoverage, EffectRegistry, EffectRow, EffectTagId,
    TraceSpecClauseFact,
};

pub type TraceSpecClauseAlternative = Vec<TraceSpecClause>;
pub type TraceSpecClauseAlternatives = Vec<TraceSpecClauseAlternative>;
/// Every attached constraint must hold; alternatives within one constraint are OR.
pub type TraceSpecConformances = Vec<TraceSpecClauseAlternatives>;

pub(super) fn normalize_alternative(
    clauses: &[TraceSpecClause],
) -> Result<
    etas_code::residual::monitor::NormalizedTraceAlternative<'_, TraceSpecPattern, (usize, Span)>,
    etas_code::residual::monitor::NormalizeError,
> {
    use etas_code::residual::monitor::{TraceClause, normalize_trace_alternative};
    normalize_trace_alternative(
        clauses
            .iter()
            .enumerate()
            .filter_map(|(index, clause)| match clause {
                TraceSpecClause::Allow { pattern, span, .. } => Some(TraceClause::Allow {
                    pattern,
                    origin: (index, *span),
                }),
                TraceSpecClause::Deny { pattern, span, .. } => Some(TraceClause::Deny {
                    pattern,
                    origin: (index, *span),
                }),
                TraceSpecClause::RequireBefore {
                    guard,
                    target,
                    span,
                    ..
                } => Some(TraceClause::Before {
                    guard,
                    target,
                    origin: (index, *span),
                }),
                TraceSpecClause::RequireAfter {
                    target,
                    obligation,
                    span,
                    ..
                } => Some(TraceClause::Before {
                    guard: target,
                    target: obligation,
                    origin: (index, *span),
                }),
                TraceSpecClause::Limit { .. } => None,
            }),
        etas_package_metadata::MAX_METADATA_GRAPH_NODES,
    )
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub enum TraceSpecClause {
    Allow {
        pattern: TraceSpecPattern,
        fact: TraceSpecClauseFact,
        span: Span,
        #[serde(default)]
        source_path: Option<Vec<u32>>,
        #[serde(default)]
        source_clause: Option<etas_code::residual::identity::ClauseId>,
    },
    Deny {
        pattern: TraceSpecPattern,
        fact: TraceSpecClauseFact,
        span: Span,
        #[serde(default)]
        source_path: Option<Vec<u32>>,
        #[serde(default)]
        source_clause: Option<etas_code::residual::identity::ClauseId>,
    },
    RequireBefore {
        guard: TraceSpecPattern,
        target: TraceSpecPattern,
        fact: TraceSpecClauseFact,
        span: Span,
        #[serde(default)]
        source_path: Option<Vec<u32>>,
        #[serde(default)]
        source_clause: Option<etas_code::residual::identity::ClauseId>,
    },
    RequireAfter {
        target: TraceSpecPattern,
        obligation: TraceSpecPattern,
        fact: TraceSpecClauseFact,
        span: Span,
        #[serde(default)]
        source_path: Option<Vec<u32>>,
        #[serde(default)]
        source_clause: Option<etas_code::residual::identity::ClauseId>,
    },
    Limit {
        fact: TraceSpecClauseFact,
    },
}

impl TraceSpecClause {
    pub fn source_clause(&self) -> Option<etas_code::residual::identity::ClauseId> {
        match self {
            Self::Allow { source_clause, .. }
            | Self::Deny { source_clause, .. }
            | Self::RequireBefore { source_clause, .. }
            | Self::RequireAfter { source_clause, .. } => *source_clause,
            Self::Limit { .. } => None,
        }
    }
    /// HIR expression-tree position, absent for metadata without source sites.
    pub fn source_path(&self) -> Option<&[u32]> {
        match self {
            Self::Allow { source_path, .. }
            | Self::Deny { source_path, .. }
            | Self::RequireBefore { source_path, .. }
            | Self::RequireAfter { source_path, .. } => source_path.as_deref(),
            Self::Limit { .. } => None,
        }
    }

    pub fn fact(&self) -> TraceSpecClauseFact {
        match self {
            Self::Allow { fact, .. }
            | Self::Deny { fact, .. }
            | Self::RequireBefore { fact, .. }
            | Self::RequireAfter { fact, .. }
            | Self::Limit { fact } => fact.clone(),
        }
    }
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct TraceSpecPattern {
    pub row: EffectRow,
    pub label: String,
    pub arg_bounds: Vec<TraceSpecArgBound>,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub struct TraceSpecArgBound {
    pub target: TraceSpecArgBoundTarget,
    pub arg_index: usize,
    pub name: String,
    pub spec_symbol: etas_hir::SymbolId,
    pub spec_args: Vec<etas_types::TypeId>,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
    etas_core::RetainedSize,
)]
pub enum TraceSpecArgBoundTarget {
    Action(ActionRef),
    EffectTag(EffectTagId),
}

impl TraceSpecPattern {
    /// A failed coverage proof is not a proof of disjointness. In particular,
    /// an unknown selector can still denote a member of a narrower pattern.
    pub(crate) fn may_match_effect(
        &self,
        effect: &Effect,
        registry: &EffectRegistry,
        types: &etas_types::TypeOutput,
    ) -> bool {
        if self.row.open.is_some() {
            return true;
        }
        let coverage = EffectCoverage {
            registry,
            types: &types.store,
        };
        let action_ref = |effect: &Effect| match effect {
            Effect::Action(action) => Some(action.clone()),
            Effect::AppliedAction(action) => Some(action.action.clone()),
            _ => None,
        };
        let actual = EffectRow::closed(crate::EffectSet::one(effect.clone()));
        self.row.effects.iter().any(|candidate| {
            if let (Some(left), Some(right)) = (action_ref(candidate), action_ref(effect))
                && left != right
            {
                return false;
            }
            if let (Effect::AppliedAction(left), Effect::AppliedAction(right)) = (candidate, effect)
                && left.action == right.action
                && let Some(signature) = registry.action_signature(&left.action)
                && left
                    .args
                    .iter()
                    .zip(&right.args)
                    .enumerate()
                    .any(|(index, (left, right))| {
                        if !matches!(
                            signature.effect_args.get(index),
                            Some(crate::EffectActionArgKind::Type)
                        ) {
                            return false;
                        }
                        let (
                            etas_types::EffectArgRef::Type(left),
                            etas_types::EffectArgRef::Type(right),
                        ) = (left, right)
                        else {
                            return false;
                        };
                        let comparable_concrete = matches!(
                            (types.store.get(*left), types.store.get(*right)),
                            (
                                Some(etas_types::Type::Nominal(_)),
                                Some(etas_types::Type::Nominal(_))
                            ) | (
                                Some(etas_types::Type::Primitive(_)),
                                Some(etas_types::Type::Primitive(_))
                            )
                        );
                        let relation = etas_types::TypeRelation::new(&types.store);
                        comparable_concrete
                            && relation.assignable(*left, *right).is_err()
                            && relation.assignable(*right, *left).is_err()
                    })
            {
                return false;
            }
            let pattern = EffectRow::closed(crate::EffectSet::one(candidate.clone()));
            !coverage.rows_disjoint(&pattern, &actual)
        })
    }

    pub fn matches_event(
        &self,
        event: &ActionEvent,
        registry: &EffectRegistry,
        types: &etas_types::TypeOutput,
    ) -> bool {
        self.matches_effect(&event.action, registry, types)
    }

    pub fn matches_effect(
        &self,
        effect: &Effect,
        registry: &EffectRegistry,
        types: &etas_types::TypeOutput,
    ) -> bool {
        let coverage = EffectCoverage {
            registry,
            types: &types.store,
        };
        self.row.effects.iter().any(|allowed| {
            coverage.covers(allowed, effect) && self.bounds_match(allowed, effect, types)
        })
    }

    fn bounds_match(
        &self,
        allowed: &Effect,
        actual: &Effect,
        types: &etas_types::TypeOutput,
    ) -> bool {
        let Some((target, actual_args)) = bound_target_and_args(allowed, actual) else {
            return self.arg_bounds.is_empty();
        };
        let spec_facts = etas_types::SpecFacts {
            canonical: types.facts.canonical_specs.clone(),
            external_signatures: types.facts.external_specs.signatures.clone(),
            external_impls: types.facts.external_specs.impls.clone(),
            external_aliases: types.facts.external_specs.aliases.clone(),
            signatures: types.facts.spec_signatures.clone(),
            impls: types.facts.spec_impls.clone(),
            type_satisfactions: types.facts.type_spec_satisfactions.clone(),
            callable_satisfactions: types.facts.callable_spec_satisfactions.clone(),
            trace_conformances: types.facts.trace_spec_conformances.clone(),
            external_callable_satisfactions: types
                .facts
                .external_callable_spec_satisfactions
                .clone(),
            external_trace_conformances: types.facts.external_trace_spec_conformances.clone(),
            type_param_bounds: types.facts.type_param_bounds.clone(),
            std_impls: types.facts.std_spec_impls.clone(),
            std_spec_aliases: types.facts.std_spec_aliases.clone(),
        };
        self.arg_bounds
            .iter()
            .filter(|bound| bound.target == target)
            .all(|bound| {
                let Some(etas_types::EffectArgRef::Type(actual_ty)) =
                    actual_args.get(bound.arg_index)
                else {
                    return false;
                };
                etas_types::solver::spec_solver::satisfies_spec(
                    &types.store,
                    &spec_facts,
                    *actual_ty,
                    bound.spec_symbol,
                    &bound.spec_args,
                )
            })
    }
}

fn bound_target_and_args<'a>(
    allowed: &Effect,
    actual: &'a Effect,
) -> Option<(TraceSpecArgBoundTarget, &'a [etas_types::EffectArgRef])> {
    match (allowed, actual) {
        (Effect::AppliedAction(allowed), Effect::AppliedAction(actual))
            if allowed.action == actual.action =>
        {
            Some((
                TraceSpecArgBoundTarget::Action(allowed.action.clone()),
                &actual.args,
            ))
        }
        (Effect::Applied { tag: allowed, .. }, Effect::Applied { tag: actual, args })
            if allowed == actual =>
        {
            let _ = args;
            None
        }
        _ => None,
    }
}

#[derive(Clone, Debug, Default, etas_core::RetainedSize)]
pub struct TraceSpecModelStore {
    pub source_clause_sites: BTreeMap<
        etas_code::residual::identity::ClauseId,
        etas_code::residual::identity::SourceClauseSite,
    >,
    pub clauses_by_item: BTreeMap<HirItemId, TraceSpecClauseAlternatives>,
    pub referenced_by_item: BTreeMap<HirItemId, TraceSpecConformances>,
    pub referenced_fact_indices: BTreeMap<HirItemId, Vec<usize>>,
    pub(crate) source_owners: BTreeMap<HirItemId, Option<CheckedTraceSourceOwner>>,
    /// Type-fact indices that failed materialization, paired with the exact
    /// error diagnostic emitted for that failure in this analysis session.
    pub(crate) rejected_fact_diagnostics: BTreeMap<usize, usize>,
    pub external_referenced_by_item: BTreeMap<ExternalTraceSpecItem, TraceSpecConformances>,
    pub trace_spec_names: BTreeMap<HirItemId, String>,
}

/// Checked source declaration identity only; this is not a residual scope,
/// activation site, contract, or installation proof.
#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub struct CheckedTraceSourceOwner {
    pub(crate) id: etas_code::residual::identity::ResidualOwnerId,
    pub(crate) declaration: etas_code::residual::identity::SourceRevisionLocation,
    pub(crate) kind: etas_code::residual::identity::ResidualOwnerKind,
}

impl CheckedTraceSourceOwner {
    pub fn id(&self) -> etas_code::residual::identity::ResidualOwnerId {
        self.id
    }
    pub fn declaration(&self) -> &etas_code::residual::identity::SourceRevisionLocation {
        &self.declaration
    }
    pub fn kind(&self) -> etas_code::residual::identity::ResidualOwnerKind {
        self.kind
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, etas_core::RetainedSize)]
pub struct ExternalTraceSpecItem {
    pub package: u32,
    pub path: Vec<String>,
}
