use std::collections::BTreeMap;

use etas_core::Span;
use etas_hir::HirItemId;

use crate::{
    ActionEvent, ActionRef, Effect, EffectCoverage, EffectRegistry, EffectRow, EffectTagId,
    TraceSpecClauseFact,
};

pub type TraceSpecClauseAlternative = Vec<TraceSpecClause>;
pub type TraceSpecClauseAlternatives = Vec<TraceSpecClauseAlternative>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceSpecClause {
    Allow {
        pattern: TraceSpecPattern,
        fact: TraceSpecClauseFact,
        span: Span,
    },
    Deny {
        pattern: TraceSpecPattern,
        fact: TraceSpecClauseFact,
        span: Span,
    },
    RequireBefore {
        guard: TraceSpecPattern,
        target: TraceSpecPattern,
        fact: TraceSpecClauseFact,
        span: Span,
    },
    RequireAfter {
        target: TraceSpecPattern,
        obligation: TraceSpecPattern,
        fact: TraceSpecClauseFact,
        span: Span,
    },
    Limit {
        fact: TraceSpecClauseFact,
    },
}

impl TraceSpecClause {
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

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TraceSpecPattern {
    pub row: EffectRow,
    pub label: String,
    pub arg_bounds: Vec<TraceSpecArgBound>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TraceSpecArgBound {
    pub target: TraceSpecArgBoundTarget,
    pub arg_index: usize,
    pub name: String,
    pub spec_symbol: etas_hir::SymbolId,
    pub spec_args: Vec<etas_types::TypeId>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TraceSpecArgBoundTarget {
    Action(ActionRef),
    EffectTag(EffectTagId),
}

impl TraceSpecPattern {
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

#[derive(Clone, Debug, Default)]
pub struct TraceSpecModelStore {
    pub clauses_by_item: BTreeMap<HirItemId, TraceSpecClauseAlternatives>,
    pub referenced_by_item: BTreeMap<HirItemId, TraceSpecClauseAlternatives>,
    pub external_referenced_by_item: BTreeMap<Vec<String>, TraceSpecClauseAlternatives>,
    pub trace_spec_names: BTreeMap<HirItemId, String>,
}
