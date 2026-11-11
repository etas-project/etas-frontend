use std::collections::BTreeMap;

use etas_utils::PartialOrder;

use crate::{
    Determinism, Effect, EffectCoverage, EffectRow, EffectSet, EffectSummary, InterpreterSupport,
    RequestInstantiationSummary, RequirementSet, ResidualCheckSet,
};

/// Non-temporal behavior. Trace placement remains in ActionTraceDomain.
#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct EffectFootprint {
    pub handler_requests: RequestInstantiationSummary,
    pub escaping_effects: EffectRow,
    pub requested_actions: EffectRow,
    pub default_actions: EffectRow,
    pub handled_actions: EffectRow,
    pub residual_checks: ResidualCheckSet,
    pub trace_spec_obligations: RequirementSet,
    pub requirements: RequirementSet,
    pub determinism: Determinism,
    pub independent_determinism: Option<Determinism>,
    pub support: InterpreterSupport,
}

impl EffectFootprint {
    pub fn from_summary(summary: &EffectSummary) -> Self {
        Self {
            handler_requests: summary.handler_requests.clone(),
            escaping_effects: summary.escaping_effects.clone(),
            requested_actions: summary.requested_actions.clone(),
            default_actions: summary.default_actions.clone(),
            handled_actions: summary.handled_actions.clone(),
            residual_checks: summary.residual_checks.clone(),
            trace_spec_obligations: summary.trace_spec_obligations.clone(),
            requirements: summary.requirements.clone(),
            determinism: summary.determinism,
            independent_determinism: summary.independent_determinism,
            support: summary.support.clone(),
        }
    }

    pub fn apply(&self, summary: &mut EffectSummary) {
        summary.handler_requests = self.handler_requests.clone();
        summary.escaping_effects = self.escaping_effects.clone();
        summary.requested_actions = self.requested_actions.clone();
        summary.default_actions = self.default_actions.clone();
        summary.handled_actions = self.handled_actions.clone();
        summary.residual_checks = self.residual_checks.clone();
        summary.trace_spec_obligations = self.trace_spec_obligations.clone();
        summary.requirements = self.requirements.clone();
        summary.determinism = self.determinism;
        summary.independent_determinism = self.independent_determinism;
        summary.support = self.support.clone();
    }

    pub fn into_summary(self) -> EffectSummary {
        let mut summary = EffectSummary::local();
        self.apply(&mut summary);
        summary
    }

    fn union_assign(&mut self, other: &Self) -> bool {
        let mut changed = self.handler_requests.union_assign(&other.handler_requests);
        changed |= self.escaping_effects.union_assign(&other.escaping_effects);
        changed |= self
            .requested_actions
            .union_assign(&other.requested_actions);
        changed |= self.default_actions.union_assign(&other.default_actions);
        changed |= self.handled_actions.union_assign(&other.handled_actions);
        changed |= self.residual_checks.union_assign(&other.residual_checks);
        changed |= self
            .trace_spec_obligations
            .union_assign(&other.trace_spec_obligations);
        changed |= self.requirements.union_assign(&other.requirements);
        changed |= self.determinism.join_assign(&other.determinism);
        changed |= super::domain::join_independent_determinism(
            &mut self.independent_determinism,
            other.independent_determinism,
        );
        changed |= self.support.join_assign(&other.support);
        changed
    }

    fn subtract_handled(
        &mut self,
        coverage: &EffectCoverage<'_>,
        handled: &EffectRow,
        intercepted: &EffectRow,
    ) {
        self.escaping_effects = coverage.subtract_handled(&self.escaping_effects, handled);
        self.handler_requests
            .subtract_handled(coverage, intercepted);
        self.default_actions = coverage.subtract_handled(&self.default_actions, intercepted);
    }
}

/// A finite guard-set domain, not a recursive copy of callable summaries.
/// The same action may also occur independently in `direct`.
#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct DefaultBehavior {
    pub direct: EffectFootprint,
    #[serde(with = "guarded_serde")]
    pub guarded: BTreeMap<EffectSet, EffectFootprint>,
}

mod guarded_serde {
    use super::*;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(
        value: &BTreeMap<EffectSet, EffectFootprint>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(value.iter())
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<EffectSet, EffectFootprint>, D::Error> {
        Vec::<(EffectSet, EffectFootprint)>::deserialize(deserializer)
            .map(|entries| entries.into_iter().collect())
    }
}

impl DefaultBehavior {
    pub fn from_summary(summary: &EffectSummary) -> Self {
        if let Some(behavior) = &summary.default_behavior {
            return (**behavior).clone();
        }
        let mut direct = EffectFootprint::from_summary(summary);
        let mut guarded = BTreeMap::new();
        if let Some(defaults) = &summary.default_handler_effects {
            direct.escaping_effects = defaults.direct.clone();
            for handler in &defaults.handlers {
                let mut footprint = EffectFootprint::from_summary(&EffectSummary::local());
                footprint.escaping_effects = handler.produced.clone();
                guarded.insert(EffectSet::one(handler.action.clone()), footprint);
            }
        }
        Self { direct, guarded }
    }

    pub fn aggregate(&self) -> EffectFootprint {
        let mut result = self.direct.clone();
        for footprint in self.guarded.values() {
            result.union_assign(footprint);
        }
        result
    }

    pub(super) fn escaping_effects(&self) -> EffectRow {
        let mut row = self.direct.escaping_effects.clone();
        for footprint in self.guarded.values() {
            row.union_assign(&footprint.escaping_effects);
        }
        row
    }

    pub fn union_assign(&mut self, other: &Self) -> bool {
        let mut changed = self.direct.union_assign(&other.direct);
        for (guards, footprint) in &other.guarded {
            changed |= self.union_guarded(guards.clone(), footprint);
        }
        changed
    }

    pub fn union_guarded(&mut self, guards: EffectSet, footprint: &EffectFootprint) -> bool {
        match self.guarded.entry(guards) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(footprint.clone());
                true
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                entry.get_mut().union_assign(footprint)
            }
        }
    }

    pub fn subtract_handled(
        &mut self,
        coverage: &EffectCoverage<'_>,
        handled: &EffectRow,
        intercepted: &EffectRow,
    ) {
        self.direct.subtract_handled(coverage, handled, intercepted);
        self.guarded.retain(|guards, footprint| {
            if guards.iter().any(|guard| {
                intercepted
                    .effects
                    .iter()
                    .any(|action| coverage.covers(action, guard))
            }) {
                return false;
            }
            footprint.subtract_handled(coverage, handled, intercepted);
            true
        });
    }

    pub fn less_equal(&self, other: &Self) -> bool {
        self.direct
            .clone()
            .into_summary()
            .less_equal(&other.direct.clone().into_summary())
            && self.guarded.iter().all(|(guards, footprint)| {
                other.guarded.get(guards).is_some_and(|other| {
                    footprint
                        .clone()
                        .into_summary()
                        .less_equal(&other.clone().into_summary())
                })
            })
    }
}

impl EffectSummary {
    pub(crate) fn join_default_behavior(&mut self, other: &Self) -> bool {
        if self.default_behavior.is_none() && other.default_behavior.is_none() {
            return false;
        }
        let mut behavior = DefaultBehavior::from_summary(self);
        let changed = behavior.union_assign(&DefaultBehavior::from_summary(other));
        let initialized = self.default_behavior.is_none();
        self.default_behavior = Some(Box::new(behavior));
        initialized || changed
    }

    pub fn record_default_behavior(
        &mut self,
        action: Effect,
        body: &Self,
    ) -> Result<(), &'static str> {
        if !action.is_action() {
            return Err("default behavior requires a checked action");
        }
        let mut behavior = DefaultBehavior::from_summary(self);
        let body_behavior = DefaultBehavior::from_summary(body);
        behavior.union_guarded(EffectSet::one(action.clone()), &body_behavior.direct);
        for (mut guards, footprint) in body_behavior.guarded {
            guards.insert(action.clone());
            behavior.union_guarded(guards, &footprint);
        }
        self.action_trace
            .seq_assign(crate::ActionTraceDomain::DefaultHandler {
                guards: EffectSet::one(action.clone()),
                body: Box::new(body.action_trace.clone()),
            });
        for handler in body
            .default_handler_effects
            .iter()
            .flat_map(|defaults| &defaults.handlers)
        {
            self.record_default_handler_effects(handler.action.clone(), EffectRow::empty())?;
        }
        self.record_default_handler_effects(action, body.escaping_effects.clone())?;
        behavior.aggregate().apply(self);
        self.default_behavior = Some(Box::new(behavior));
        Ok(())
    }

    pub(crate) fn with_direct_behavior(&mut self, apply: impl FnOnce(&mut EffectSummary)) {
        if let Some(behavior) = &mut self.default_behavior {
            let mut direct = behavior.direct.clone().into_summary();
            apply(&mut direct);
            behavior.direct = EffectFootprint::from_summary(&direct);
        }
    }

    pub(crate) fn map_behavior_parts(&mut self, mut apply: impl FnMut(&mut EffectSummary)) {
        if let Some(behavior) = &mut self.default_behavior {
            for footprint in
                std::iter::once(&mut behavior.direct).chain(behavior.guarded.values_mut())
            {
                let mut part = footprint.clone().into_summary();
                apply(&mut part);
                *footprint = EffectFootprint::from_summary(&part);
            }
        }
    }

    pub(crate) fn record_requirements(
        &mut self,
        requirements: &RequirementSet,
        obligations: &RequirementSet,
    ) {
        self.with_direct_behavior(|summary| summary.record_requirements(requirements, obligations));
        self.requirements.union_assign(requirements);
        self.trace_spec_obligations.union_assign(obligations);
    }

    pub(crate) fn refresh_default_behavior(&mut self) {
        if let Some(behavior) = &self.default_behavior {
            let footprint = behavior.aggregate();
            footprint.apply(self);
        }
    }

    pub(crate) fn merge_default_footprint(&mut self, other: &Self) -> bool {
        if self.default_behavior.is_none() && other.default_behavior.is_none() {
            return false;
        }
        let mut behavior = DefaultBehavior::from_summary(self);
        let mut other = DefaultBehavior::from_summary(other);
        other.direct.escaping_effects = EffectRow::empty();
        for footprint in other.guarded.values_mut() {
            footprint.escaping_effects = EffectRow::empty();
        }
        let changed = behavior.union_assign(&other);
        let initialized = self.default_behavior.is_none();
        self.default_behavior = Some(Box::new(behavior));
        initialized || changed
    }
}

pub(super) fn subtract_default_trace(
    trace: &crate::ActionTraceDomain,
    coverage: &EffectCoverage<'_>,
    handled: &EffectRow,
) -> Result<crate::ActionTraceDomain, String> {
    struct Filter<'a, 'b> {
        coverage: &'a EffectCoverage<'b>,
        handled: &'a EffectRow,
    }
    impl crate::substitution::SummaryMapper for Filter<'_, '_> {
        type Error = String;
        fn checkpoint(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn invalid_structure(&self) -> String {
            "invalid default-handler trace structure".into()
        }
        fn unsupported_residual_substitution(&self) -> String {
            "unexpected residual substitution".into()
        }
        fn ty(&mut self, ty: etas_types::TypeId) -> Result<etas_types::TypeId, String> {
            Ok(ty)
        }
        fn effect(&mut self, effect: Effect) -> Result<Effect, String> {
            Ok(effect)
        }
        fn row(&mut self, row: &EffectRow) -> Result<EffectRow, String> {
            Ok(row.clone())
        }
        fn default_guards(&mut self, guards: &EffectSet) -> Result<Option<EffectSet>, String> {
            Ok((!guards.iter().any(|guard| {
                self.handled
                    .effects
                    .iter()
                    .any(|action| self.coverage.covers(action, guard))
            }))
            .then(|| guards.clone()))
        }
    }
    crate::substitution::map_trace(&mut Filter { coverage, handled }, trace)
}
