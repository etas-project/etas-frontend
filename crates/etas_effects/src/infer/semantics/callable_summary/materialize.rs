use std::collections::BTreeSet;

use super::{super::engine::EffectSemantics, LatentSource};
use crate::{CallableReturnSource, EffectPipelineError, EffectUnit, facts::CapturedCallableId};

mod descriptors;
mod metadata;

impl EffectSemantics<'_> {
    pub(crate) fn materialize_captured_callable_analyses(
        &mut self,
    ) -> Result<(), EffectPipelineError> {
        let mut instances = BTreeSet::new();
        let mut native_origins = std::collections::BTreeMap::new();
        for unit in self.inputs.unit_effects.keys() {
            if let EffectUnit::AnonymousFlow { value, .. } = unit {
                let origin = self.callable_environment_site(*value).map_err(|error| {
                    EffectPipelineError::InvalidCallableFacts {
                        reason: error.to_string(),
                    }
                })?;
                if native_origins
                    .insert(origin, *unit)
                    .is_some_and(|previous| previous != *unit)
                {
                    return Err(EffectPipelineError::InvalidCallableFacts {
                        reason: "anonymous callable creation sites have ambiguous identity".into(),
                    });
                }
            }
        }
        for environment in self.callable_environments.borrow().keys() {
            if let Some(unit) = native_origins.get(&environment.origin) {
                instances.insert(CapturedCallableId {
                    unit: *unit,
                    environment: *environment,
                });
            }
        }
        for key in self.callable_summaries.borrow().summaries.keys() {
            if let LatentSource::FrozenContract {
                unit,
                reference: None,
                environment: Some(environment),
            }
            | LatentSource::Captured { unit, environment } = key.source
            {
                instances.insert(CapturedCallableId { unit, environment });
            }
        }
        for environment in self.callable_environments.borrow().values() {
            for source in environment.parameters.values().flatten() {
                if let LatentSource::FrozenContract {
                    unit,
                    reference: None,
                    environment: Some(environment),
                }
                | LatentSource::Captured { unit, environment } = source
                {
                    instances.insert(CapturedCallableId {
                        unit: *unit,
                        environment: *environment,
                    });
                }
            }
        }
        for summary in self.inputs.unit_effects.values() {
            for exit in summary.exits.iter().flat_map(|exits| &exits.exits) {
                if let crate::CallableOutcome::Returned {
                    observation: crate::ReturnObservation::CallableSources(sources),
                    ..
                } = &exit.outcome
                {
                    for source in sources {
                        if let CallableReturnSource::FrozenContract {
                            unit,
                            reference: None,
                            environment: Some(environment),
                        }
                        | CallableReturnSource::Captured { unit, environment } = source
                        {
                            instances.insert(CapturedCallableId {
                                unit: *unit,
                                environment: *environment,
                            });
                        }
                    }
                }
            }
        }
        self.callable_summaries.get_mut().invalidate();
        self.materialize_callable_descriptors(instances)
    }
}
