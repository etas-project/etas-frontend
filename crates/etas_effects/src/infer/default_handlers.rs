use crate::{Effect, EffectCoverage, EffectRow, EffectSet, EffectSummary};

#[cfg(test)]
#[path = "default_handlers/tests.rs"]
mod tests;

/// Keep the cause of default-provider effects until the surrounding handlers
/// have been interpreted. Equal error types can have independent source causes.
#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct DefaultHandlerEffects {
    pub direct: EffectRow,
    pub handlers: Vec<DefaultHandlerContribution>,
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub struct DefaultHandlerContribution {
    pub action: Effect,
    pub produced: EffectRow,
}

impl DefaultHandlerEffects {
    pub(crate) fn union_handler(&mut self, action: Effect, produced: &EffectRow) -> bool {
        if let Some(existing) = self
            .handlers
            .iter_mut()
            .find(|handler| handler.action == action)
        {
            return existing.produced.union_assign(produced);
        }
        self.handlers.push(DefaultHandlerContribution {
            action,
            produced: produced.clone(),
        });
        self.handlers.sort_by(|a, b| a.action.cmp(&b.action));
        true
    }
}

impl EffectSummary {
    pub fn direct_escaping_effects(&self) -> &EffectRow {
        if let Some(behavior) = &self.default_behavior {
            return &behavior.direct.escaping_effects;
        }
        self.default_handler_effects
            .as_ref()
            .map_or(&self.escaping_effects, |effects| &effects.direct)
    }

    pub fn record_default_handler_effects(
        &mut self,
        action: Effect,
        produced: EffectRow,
    ) -> Result<(), &'static str> {
        if !action.is_action() {
            return Err("default handler contribution must identify a checked action");
        }
        if let Some(behavior) = &mut self.default_behavior {
            let mut footprint = super::EffectFootprint::from_summary(&EffectSummary::local());
            footprint.escaping_effects = produced.clone();
            behavior.union_guarded(EffectSet::one(action.clone()), &footprint);
        }
        let effects = self
            .default_handler_effects
            .get_or_insert_with(|| DefaultHandlerEffects {
                direct: self.escaping_effects.clone(),
                handlers: vec![],
            });
        effects.union_handler(action, &produced);
        self.refresh_escaping_effects();
        Ok(())
    }

    pub(crate) fn join_escaping_effects(&mut self, other: &Self) -> bool {
        if self.default_handler_effects.is_none() && other.default_handler_effects.is_none() {
            return self.escaping_effects.union_assign(&other.escaping_effects);
        }
        let initialized = self.default_handler_effects.is_none();
        let effects = self
            .default_handler_effects
            .get_or_insert_with(|| DefaultHandlerEffects {
                direct: self.escaping_effects.clone(),
                handlers: vec![],
            });
        let mut changed = initialized;
        changed |= effects.direct.union_assign(other.direct_escaping_effects());
        if let Some(other) = &other.default_handler_effects {
            for handler in &other.handlers {
                changed |= effects.union_handler(handler.action.clone(), &handler.produced);
            }
        }
        self.refresh_escaping_effects();
        changed
    }

    pub fn subtract_handled_effects(
        &mut self,
        coverage: &EffectCoverage<'_>,
        handled: &EffectRow,
    ) -> Result<(), String> {
        self.apply_handler_coverage(coverage, handled, handled)
    }

    pub(crate) fn apply_handler_coverage(
        &mut self,
        coverage: &EffectCoverage<'_>,
        handled: &EffectRow,
        intercepted: &EffectRow,
    ) -> Result<(), String> {
        self.action_trace = super::default_behavior::subtract_default_trace(
            &self.action_trace,
            coverage,
            intercepted,
        )?;
        if let Some(behavior) = &mut self.default_behavior {
            behavior.subtract_handled(coverage, handled, intercepted);
        }
        self.refresh_default_behavior();
        self.handler_requests
            .subtract_handled(coverage, intercepted);
        if let Some(effects) = &mut self.default_handler_effects {
            effects.direct = coverage.subtract_handled(&effects.direct, handled);
            effects.handlers.retain_mut(|handler| {
                let request = EffectRow::closed(EffectSet::one(handler.action.clone()));
                if coverage.subtract_handled(&request, intercepted).is_empty() {
                    return false;
                }
                handler.produced = coverage.subtract_handled(&handler.produced, handled);
                true
            });
            if effects.handlers.is_empty() {
                self.escaping_effects = effects.direct.clone();
                self.default_handler_effects = None;
                self.refresh_default_behavior();
                return Ok(());
            }
            self.refresh_escaping_effects();
        } else {
            self.escaping_effects = coverage.subtract_handled(&self.escaping_effects, handled);
        }
        Ok(())
    }

    pub(crate) fn remove_escaping_effect(&mut self, effect: &Effect) {
        if let Some(behavior) = &mut self.default_behavior {
            behavior.direct.escaping_effects.remove_effect(effect);
            for footprint in behavior.guarded.values_mut() {
                footprint.escaping_effects.remove_effect(effect);
            }
        }
        if let Some(effects) = &mut self.default_handler_effects {
            effects.direct.remove_effect(effect);
            for handler in &mut effects.handlers {
                handler.produced.remove_effect(effect);
            }
            self.refresh_escaping_effects();
        } else {
            self.escaping_effects.remove_effect(effect);
        }
    }

    pub fn refresh_escaping_effects(&mut self) {
        if let Some(behavior) = &self.default_behavior {
            self.escaping_effects = behavior.escaping_effects();
            return;
        }
        if let Some(effects) = &self.default_handler_effects {
            let mut row = effects.direct.clone();
            for handler in &effects.handlers {
                row.union_assign(&handler.produced);
            }
            self.escaping_effects = row;
        }
    }
}
