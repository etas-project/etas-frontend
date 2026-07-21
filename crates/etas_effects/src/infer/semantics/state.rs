use std::collections::{BTreeMap, BTreeSet};

use etas_hir::{HirExprId, HirItemId, SymbolId};
use etas_utils::{JoinSemiLattice, PartialOrder};

use crate::{EffectSummary, EffectUnit, FrontendRejectionReason, InterpreterSupport};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LatentEnv {
    pub(crate) values: BTreeMap<HirExprId, BTreeSet<EffectUnit>>,
    pub(crate) symbols: BTreeMap<SymbolId, BTreeSet<EffectUnit>>,
}

impl LatentEnv {
    pub(crate) fn record_value(&mut self, value: HirExprId, unit: EffectUnit) {
        self.values.entry(value).or_default().insert(unit);
    }

    pub(crate) fn record_symbol_sources(
        &mut self,
        symbol: SymbolId,
        sources: impl IntoIterator<Item = EffectUnit>,
    ) {
        self.symbols.entry(symbol).or_default().extend(sources);
    }

    pub(crate) fn value_sources(&self, value: HirExprId) -> Option<&BTreeSet<EffectUnit>> {
        self.values.get(&value)
    }

    pub(crate) fn symbol_sources(&self, symbol: SymbolId) -> Option<&BTreeSet<EffectUnit>> {
        self.symbols.get(&symbol)
    }

    pub(crate) fn join_assign(&mut self, other: &Self) -> bool {
        let mut changed = false;
        for (value, sources) in &other.values {
            let entry = self.values.entry(*value).or_default();
            let old_len = entry.len();
            entry.extend(sources.iter().copied());
            changed |= entry.len() != old_len;
        }
        for (symbol, sources) in &other.symbols {
            let entry = self.symbols.entry(*symbol).or_default();
            let old_len = entry.len();
            entry.extend(sources.iter().copied());
            changed |= entry.len() != old_len;
        }
        changed
    }

    fn less_equal(&self, other: &Self) -> bool {
        map_set_less_equal(&self.values, &other.values)
            && map_set_less_equal(&self.symbols, &other.symbols)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct EffectState {
    pub(crate) unit: Option<EffectUnit>,
    pub(crate) owner: Option<HirItemId>,
    pub(crate) summary: EffectSummary,
    pub(crate) local_latent_values: LatentEnv,
    pub(crate) local_latent_containers: LatentEnv,
    pub(crate) handler_depth: usize,
    pub(crate) latent_definition_depth: usize,
    pub(crate) resume_count: usize,
    pub(crate) incomplete: bool,
}

impl EffectState {
    pub(crate) fn for_unit(unit: EffectUnit) -> Self {
        Self {
            owner: unit.owner(),
            unit: Some(unit),
            ..Self::default()
        }
    }

    pub(crate) fn mark_incomplete(&mut self) {
        self.incomplete = true;
        self.summary
            .support
            .join_assign(&InterpreterSupport::Rejected(
                FrontendRejectionReason::UnresolvedEffect,
            ));
    }
}

impl PartialOrder for EffectState {
    fn less_equal(&self, other: &Self) -> bool {
        option_less_equal(&self.unit, &other.unit)
            && option_less_equal(&self.owner, &other.owner)
            && self.summary.less_equal(&other.summary)
            && self
                .local_latent_values
                .less_equal(&other.local_latent_values)
            && self
                .local_latent_containers
                .less_equal(&other.local_latent_containers)
            && self.handler_depth == other.handler_depth
            && self.latent_definition_depth == other.latent_definition_depth
            && self.resume_count == other.resume_count
            && (!self.incomplete || other.incomplete)
    }
}

impl JoinSemiLattice for EffectState {
    fn bottom() -> Self {
        Self::default()
    }

    fn join_assign(&mut self, other: &Self) -> bool {
        if self.is_bottom_like() {
            let changed = self != other;
            *self = other.clone();
            return changed;
        }
        if other.is_bottom_like() {
            return false;
        }
        let mut changed = false;
        let unit_conflict = option_conflict(self.unit, other.unit);
        let owner_conflict = option_conflict(self.owner, other.owner);
        changed |= join_option(&mut self.unit, other.unit);
        changed |= join_option(&mut self.owner, other.owner);
        changed |= self.summary.join_branch(&other.summary);
        changed |= self
            .local_latent_values
            .join_assign(&other.local_latent_values);
        changed |= self
            .local_latent_containers
            .join_assign(&other.local_latent_containers);
        if unit_conflict
            || owner_conflict
            || self.handler_depth != other.handler_depth
            || self.latent_definition_depth != other.latent_definition_depth
            || self.resume_count != other.resume_count
        {
            let was_incomplete = self.incomplete;
            self.handler_depth = self.handler_depth.max(other.handler_depth);
            self.latent_definition_depth = self
                .latent_definition_depth
                .max(other.latent_definition_depth);
            self.resume_count = self.resume_count.max(other.resume_count);
            self.mark_incomplete();
            changed |= self.incomplete != was_incomplete;
        }
        let old_incomplete = self.incomplete;
        self.incomplete |= other.incomplete;
        changed |= old_incomplete != self.incomplete;
        changed
    }
}

impl EffectState {
    fn is_bottom_like(&self) -> bool {
        self.unit.is_none()
            && self.owner.is_none()
            && self.summary == EffectSummary::default()
            && self.local_latent_values == LatentEnv::default()
            && self.local_latent_containers == LatentEnv::default()
            && self.handler_depth == 0
            && self.latent_definition_depth == 0
            && self.resume_count == 0
            && !self.incomplete
    }
}

impl EffectUnit {
    pub fn owner(self) -> Option<HirItemId> {
        match self {
            Self::Item(item)
            | Self::HandlerArm { owner: item, .. }
            | Self::AnonymousFlow { owner: item, .. }
            | Self::FirstClassFlowCall { owner: item, .. } => Some(item),
        }
    }
}

fn option_less_equal<T>(left: &Option<T>, right: &Option<T>) -> bool
where
    T: Eq,
{
    left == right || left.is_none()
}

fn join_option<T>(slot: &mut Option<T>, other: Option<T>) -> bool
where
    T: Copy + Eq,
{
    match (*slot, other) {
        (None, Some(other)) => {
            *slot = Some(other);
            true
        }
        (Some(left), Some(right)) if left != right => false,
        _ => false,
    }
}

fn option_conflict<T>(left: Option<T>, right: Option<T>) -> bool
where
    T: Eq,
{
    matches!((left, right), (Some(left), Some(right)) if left != right)
}

fn map_set_less_equal<K, V>(
    left: &BTreeMap<K, BTreeSet<V>>,
    right: &BTreeMap<K, BTreeSet<V>>,
) -> bool
where
    K: Ord,
    V: Ord,
{
    left.iter()
        .all(|(key, values)| right.get(key).is_some_and(|other| values.is_subset(other)))
}
