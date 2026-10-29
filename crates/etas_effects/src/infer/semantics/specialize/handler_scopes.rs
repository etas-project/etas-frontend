use etas_core::Span;
use etas_hir::HirExprId;

use super::{EffectSemantics, EffectSpecializationError};
use crate::{Effect, EffectCoverage, EffectRow, EffectSet, EffectSummary, ParameterCallTarget};

impl EffectSemantics<'_> {
    pub(super) fn remove_intercepted_callback_defaults(
        &self,
        target: &EffectSummary,
        parameter: ParameterCallTarget,
        span: Span,
        callback: &mut EffectSummary,
    ) -> Result<(), EffectSpecializationError> {
        let intercepted =
            self.definitely_intercepted_callback_defaults(target, parameter, span, callback)?;
        if !intercepted.is_empty() {
            let coverage = EffectCoverage {
                registry: self.registry,
                types: &self.types.store,
            };
            callback.subtract_handled_effects(&coverage, &intercepted);
            callback.default_actions =
                coverage.subtract_handled(&callback.default_actions, &intercepted);
        }
        Ok(())
    }

    pub(super) fn definitely_intercepted_callback_defaults(
        &self,
        target: &EffectSummary,
        parameter: ParameterCallTarget,
        span: Span,
        callback: &EffectSummary,
    ) -> Result<EffectRow, EffectSpecializationError> {
        let Some(exits) = &target.exits else {
            return Ok(EffectRow::empty());
        };
        let matching = exits
            .parameter_exits
            .iter()
            .filter(|exit| exit.target == parameter && exit.source.span() == span)
            .collect::<Vec<_>>();
        if matching.is_empty() {
            return Ok(EffectRow::empty());
        }
        let coverage = EffectCoverage {
            registry: self.registry,
            types: &self.types.store,
        };
        let mut handled = EffectSet::new();
        for action in callback.default_actions.effects.iter() {
            let mut every_path = true;
            for exit in &matching {
                let Some(scopes) = &exit.enclosing_handlers else {
                    every_path = false;
                    break;
                };
                let mut intercepted = false;
                for handler in scopes {
                    if self.handler_unconditionally_intercepts(*handler, action, &coverage)? {
                        intercepted = true;
                        break;
                    }
                }
                if !intercepted {
                    every_path = false;
                    break;
                }
            }
            if every_path {
                handled.insert(action.clone());
            }
        }
        Ok(EffectRow::closed(handled))
    }

    fn handler_unconditionally_intercepts(
        &self,
        handler: HirExprId,
        action: &Effect,
        coverage: &EffectCoverage<'_>,
    ) -> Result<bool, EffectSpecializationError> {
        let handled = self
            .definitely_handled_actions(handler)
            .map_err(EffectSpecializationError::Interrupted)?;
        Ok(handled
            .effects
            .iter()
            .any(|effect| coverage.covers(effect, action)))
    }
}
