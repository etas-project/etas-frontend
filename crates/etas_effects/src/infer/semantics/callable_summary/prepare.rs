use super::*;
use crate::infer::semantics::engine::EffectSemantics;

impl EffectSemantics<'_> {
    pub(super) fn prepare_callable_summary(
        &self,
        key: CallableSummaryKey,
    ) -> Result<CallableSummaryPlan, EffectSpecializationError> {
        match key.source {
            LatentSource::MetadataSpecMethod {
                invocation,
                environment,
            } => match self.metadata_spec_method_evidence(invocation, environment)? {
                super::super::spec_methods::MethodEvidence::Resolved(LatentSource::Captured {
                    unit,
                    environment,
                }) => self.prepare_native_value_summary(unit, environment, key.span(), key.mode),
                super::super::spec_methods::MethodEvidence::Deferred { summary } => {
                    Ok(CallableSummaryPlan::Leaf(*summary))
                }
                super::super::spec_methods::MethodEvidence::Resolved(LatentSource::Metadata(
                    source,
                )) => self.prepare_metadata_value_summary(source, key.span(), key.mode),
                _ => Err(invalid(
                    "metadata method evidence has no checked callable implementation",
                )),
            },
            LatentSource::SpecMethod { call, environment } => {
                match self.spec_method_evidence(call, environment)? {
                    super::super::spec_methods::MethodEvidence::Resolved(
                        LatentSource::Captured { unit, environment },
                    ) => self.prepare_native_value_summary(unit, environment, key.span(), key.mode),
                    super::super::spec_methods::MethodEvidence::Deferred { summary, .. } => {
                        Ok(CallableSummaryPlan::Leaf(*summary))
                    }
                    super::super::spec_methods::MethodEvidence::Resolved(
                        LatentSource::Metadata(source),
                    ) => self.prepare_metadata_value_summary(source, key.span(), key.mode),
                    _ => Err(invalid(
                        "method evidence did not resolve to a checked implementation environment",
                    )),
                }
            }
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
