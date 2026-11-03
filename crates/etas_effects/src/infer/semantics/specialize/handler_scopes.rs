use super::{EffectSemantics, EffectSpecializationError};
use crate::{
    EffectCoverage, EffectRow, EffectSet, EffectSummary, ParameterCallTarget,
    RequestInstantiationSummary,
};
use etas_core::Span;

impl EffectSemantics<'_> {
    pub(super) fn remove_intercepted_callback_defaults(
        &self,
        target: &mut EffectSummary,
        parameter: ParameterCallTarget,
        span: Span,
        callback: &mut EffectSummary,
    ) -> Result<(), EffectSpecializationError> {
        let intercepted =
            self.definitely_intercepted_callback_defaults(target, parameter, span, callback)?;
        if !intercepted.is_empty() {
            target.record_handled_action_row(intercepted.clone());
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
        let RequestInstantiationSummary::Available { parameters, .. } = &target.handler_requests
        else {
            return Ok(EffectRow::empty());
        };
        let matching = parameters
            .iter()
            .filter(|request| request.target == parameter && request.span == span)
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
            for request in &matching {
                if let Some(control) = self.query_control {
                    control
                        .checkpoint()
                        .map_err(EffectSpecializationError::Interrupted)?;
                }
                if !request
                    .excluded
                    .iter()
                    .any(|handled| coverage.covers(handled, action))
                {
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
}
