use std::collections::{BTreeMap, BTreeSet};

use etas_hir::{HirExprId, HirItemId, SymbolId};
use etas_utils::{JoinSemiLattice, PartialOrder};

use crate::{EffectSummary, EffectUnit, FrontendRejectionReason, InterpreterSupport};

pub(crate) use crate::facts::CallableValueOrigin as LatentSource;

impl LatentSource {
    pub(crate) fn captured_call_target(&self) -> Option<EffectUnit> {
        match self {
            Self::FrozenContract { unit, .. } | Self::Unit(unit) | Self::Reference { unit, .. } => Some(*unit),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LatentEnv {
    pub(crate) values: BTreeMap<HirExprId, BTreeSet<LatentSource>>,
    pub(crate) symbols: BTreeMap<SymbolId, BTreeSet<LatentSource>>,
    pub(crate) value_projections: BTreeMap<
        (HirExprId, Vec<etas_code::descriptor::ValueProjectionStep>),
        BTreeSet<LatentSource>,
    >,
    pub(crate) symbol_projections: BTreeMap<
        (SymbolId, Vec<etas_code::descriptor::ValueProjectionStep>),
        BTreeSet<LatentSource>,
    >,
}

/// Size the discovered origin graph as entries change during a solver sweep.
#[derive(Default)]
pub(crate) struct DiscoveredCallResults {
    pub(crate) origins: LatentEnv,
    pub(crate) value_nodes: usize,
    pub(crate) projection_nodes: usize,
}

impl DiscoveredCallResults {
    pub(super) fn retain_values(&mut self, clean: impl Fn(&HirExprId) -> bool) {
        self.origins.values.retain(|key, _| clean(key));
        self.origins.value_projections.retain(|(key, _), _| clean(key));
        self.value_nodes = self.origins.values.values().map(|sources| 1 + sources.len()).sum();
        self.projection_nodes = self.origins.value_projections.iter().map(|((_, path), sources)| 1 + path.len() + sources.len()).sum();
    }
    pub(super) fn update_value(
        &mut self,
        expr: HirExprId,
        update: impl FnOnce(&mut BTreeSet<LatentSource>),
    ) {
        let values = self.origins.values.entry(expr).or_insert_with(|| {
            self.value_nodes += 1;
            BTreeSet::new()
        });
        let before = values.len();
        update(values);
        self.value_nodes = self.value_nodes - before + values.len();
    }
    pub(super) fn remove_value(&mut self, expr: HirExprId) {
        if let Some(sources) = self.origins.values.remove(&expr) {
            self.value_nodes -= 1 + sources.len();
        }
    }
    pub(super) fn extend_projection(
        &mut self,
        key: (HirExprId, Vec<etas_code::descriptor::ValueProjectionStep>),
        sources: impl IntoIterator<Item = LatentSource>,
    ) {
        let nodes = 1 + key.1.len();
        let values = self
            .origins
            .value_projections
            .entry(key)
            .or_insert_with(|| {
                self.projection_nodes += nodes;
                BTreeSet::new()
            });
        let before = values.len();
        values.extend(sources);
        self.projection_nodes += values.len() - before;
    }
}

impl LatentEnv {
    pub(crate) fn record_value_sources(
        &mut self,
        value: HirExprId,
        sources: impl IntoIterator<Item = LatentSource>,
    ) {
        self.values.entry(value).or_default().extend(sources);
    }
    pub(crate) fn record_value(&mut self, value: HirExprId, unit: EffectUnit) {
        self.values
            .entry(value)
            .or_default()
            .insert(LatentSource::Unit(unit));
    }

    pub(crate) fn record_symbol_sources(
        &mut self,
        symbol: SymbolId,
        sources: impl IntoIterator<Item = LatentSource>,
    ) {
        self.symbols.entry(symbol).or_default().extend(sources);
    }

    pub(crate) fn set_symbol_sources(
        &mut self,
        symbol: SymbolId,
        sources: impl IntoIterator<Item = LatentSource>,
    ) {
        self.symbols.insert(symbol, sources.into_iter().collect());
        self.symbol_projections
            .retain(|(owner, _), _| *owner != symbol);
    }

    pub(crate) fn value_sources(&self, value: HirExprId) -> Option<&BTreeSet<LatentSource>> {
        self.values.get(&value)
    }

    pub(crate) fn symbol_sources(&self, symbol: SymbolId) -> Option<&BTreeSet<LatentSource>> {
        self.symbols.get(&symbol)
    }

    pub(crate) fn join_assign(&mut self, other: &Self) -> bool {
        let mut changed = false;
        for (value, sources) in &other.values {
            changed |= !self.values.contains_key(value);
            let entry = self.values.entry(*value).or_default();
            let old_len = entry.len();
            entry.extend(sources.iter().copied());
            changed |= entry.len() != old_len;
        }
        for (symbol, sources) in &other.symbols {
            changed |= !self.symbols.contains_key(symbol);
            let entry = self.symbols.entry(*symbol).or_default();
            let old_len = entry.len();
            entry.extend(sources.iter().copied());
            changed |= entry.len() != old_len;
        }
        for (position, sources) in &other.value_projections {
            changed |= !self.value_projections.contains_key(position);
            let entry = self.value_projections.entry(position.clone()).or_default();
            let before = entry.len();
            entry.extend(sources.iter().copied());
            changed |= before != entry.len();
        }
        for (position, sources) in &other.symbol_projections {
            changed |= !self.symbol_projections.contains_key(position);
            let entry = self.symbol_projections.entry(position.clone()).or_default();
            let before = entry.len();
            entry.extend(sources.iter().copied());
            changed |= before != entry.len();
        }
        changed
    }

    fn less_equal(&self, other: &Self) -> bool {
        map_set_less_equal(&self.values, &other.values)
            && map_set_less_equal(&self.symbols, &other.symbols)
            && map_set_less_equal(&self.value_projections, &other.value_projections)
            && map_set_less_equal(&self.symbol_projections, &other.symbol_projections)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct EffectState {
    pub(crate) unit: Option<EffectUnit>,
    pub(crate) owner: Option<HirItemId>,
    pub(crate) application: Option<etas_code::CodeFingerprint>,
    pub(crate) summary: EffectSummary,
    pub(crate) local_latent_values: LatentEnv,
    pub(crate) local_latent_containers: LatentEnv,
    pub(crate) agent_state: BTreeMap<
        (HirExprId, Vec<etas_code::descriptor::ValueProjectionStep>),
        BTreeSet<LatentSource>,
    >,
    pub(crate) handler_depth: usize,
    pub(crate) latent_definition_depth: usize,
    pub(crate) resume_count: usize,
    pub(crate) incomplete: bool,
    pub(crate) exits: crate::CallableExitSummary,
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
        self.summary.with_direct_behavior(|summary| {
            summary.support.join_assign(&InterpreterSupport::Rejected(
                FrontendRejectionReason::UnresolvedEffect,
            ));
        });
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
            && option_less_equal(&self.application, &other.application)
            && self.summary.less_equal(&other.summary)
            && self
                .local_latent_values
                .less_equal(&other.local_latent_values)
            && self
                .local_latent_containers
                .less_equal(&other.local_latent_containers)
            && map_set_less_equal(&self.agent_state, &other.agent_state)
            && self.handler_depth == other.handler_depth
            && self.latent_definition_depth == other.latent_definition_depth
            && self.resume_count <= other.resume_count
            && (!self.incomplete || other.incomplete)
            && self.exits == other.exits
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
        changed |= self.exits.join_assign(&other.exits);
        let unit_conflict = option_conflict(self.unit, other.unit);
        let owner_conflict = option_conflict(self.owner, other.owner);
        let application_conflict = option_conflict(self.application, other.application);
        changed |= join_option(&mut self.unit, other.unit);
        changed |= join_option(&mut self.owner, other.owner);
        changed |= join_option(&mut self.application, other.application);
        changed |= self.summary.join_branch(&other.summary);
        changed |= self
            .local_latent_values
            .join_assign(&other.local_latent_values);
        changed |= self
            .local_latent_containers
            .join_assign(&other.local_latent_containers);
        for (slot, incoming) in &other.agent_state {
            changed |= !self.agent_state.contains_key(slot);
            let origins = self.agent_state.entry(slot.clone()).or_default();
            let before = origins.len();
            origins.extend(incoming);
            changed |= origins.len() != before;
        }
        let resumes = self.resume_count.max(other.resume_count);
        changed |= resumes != self.resume_count;
        self.resume_count = resumes;
        if unit_conflict
            || owner_conflict
            || application_conflict
            || self.handler_depth != other.handler_depth
            || self.latent_definition_depth != other.latent_definition_depth
        {
            let was_incomplete = self.incomplete;
            self.handler_depth = self.handler_depth.max(other.handler_depth);
            self.latent_definition_depth = self
                .latent_definition_depth
                .max(other.latent_definition_depth);
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
            && self.application.is_none()
            && self.summary == EffectSummary::default()
            && self.local_latent_values == LatentEnv::default()
            && self.local_latent_containers == LatentEnv::default()
            && self.agent_state.is_empty()
            && self.handler_depth == 0
            && self.latent_definition_depth == 0
            && self.resume_count == 0
            && !self.incomplete
            && self.exits == crate::CallableExitSummary::default()
    }
}

impl EffectUnit {
    pub fn owner(self) -> Option<HirItemId> {
        match self {
            Self::Item(item)
            | Self::HandlerArm { owner: item, .. }
            | Self::AnonymousFlow { owner: item, .. } => Some(item),
            Self::ParameterDefault { owner, .. } => Some(owner),
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

#[cfg(test)]
mod tests;
