use super::*;
use crate::infer::semantics::engine::EffectSemantics;

impl EffectSemantics<'_> {
    pub(super) fn prepare_callable_summary(
        &self,
        key: CallableSummaryKey,
    ) -> Result<CallableSummaryPlan, EffectSpecializationError> {
        let source = match key.source {
            LatentSource::FrozenContract {
                unit,
                reference,
                environment,
            } => match (reference, environment) {
                (Some(value), environment) => LatentSource::Reference {
                    unit,
                    value,
                    environment,
                },
                (None, Some(environment)) => LatentSource::Captured { unit, environment },
                (None, None) => LatentSource::Unit(unit),
            },
            source => source,
        };
        match source {
            LatentSource::Captured { unit, environment } => {
                self.prepare_native_value_summary(unit, environment, key.span(), key.mode)
            }
            LatentSource::Metadata(source) => {
                self.prepare_metadata_value_summary(source, key.span(), key.mode)
            }
            source => {
                let template = self
                    .latent_leaf_summary(source, key.span())
                    .ok_or_else(|| {
                        invalid(&format!(
                            "callable dependency {source:?} has no solved checked summary"
                        ))
                    })?;
                Ok(CallableSummaryPlan::Leaf(template))
            }
        }
    }
}
