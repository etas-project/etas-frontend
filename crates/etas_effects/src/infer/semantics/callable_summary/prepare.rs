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
            LatentSource::ExternalFrozen { reflection } => self
                .external_frozen_summary(reflection, key.span())
                .map(CallableSummaryPlan::Leaf),
            LatentSource::ExternalReference { value, .. } => self
                .external_reference_summary(value, key.span())
                .map(CallableSummaryPlan::Leaf),
            LatentSource::MetadataSpecMethod {
                invocation,
                environment,
            } => match self.metadata_spec_method_evidence(invocation, environment)? {
                super::super::spec_methods::MethodEvidence::Resolved(LatentSource::Captured {
                    unit,
                    environment,
                }) => self.prepare_native_value_summary(unit, environment, key.span(), key.mode),
                super::super::spec_methods::MethodEvidence::Deferred { summary }
                | super::super::spec_methods::MethodEvidence::Contract { summary } => {
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
                use super::super::spec_methods::MethodEvidence;
                let managed = self.is_bound_agent_request(call);
                match self.spec_method_evidence(call, environment)? {
                    MethodEvidence::Alternatives(sources) => Ok(CallableSummaryPlan::Dispatch {
                        implementations: sources
                            .into_iter()
                            .map(|source| CallableSummaryKey::in_mode(source, key.span(), key.mode))
                            .collect(),
                        managed,
                    }),
                    MethodEvidence::Resolved(source) if managed => {
                        Ok(CallableSummaryPlan::Dispatch {
                            implementations: [CallableSummaryKey::in_mode(
                                source,
                                key.span(),
                                key.mode,
                            )]
                            .into(),
                            managed,
                        })
                    }
                    super::super::spec_methods::MethodEvidence::Resolved(
                        LatentSource::Captured { unit, environment },
                    ) => self.prepare_native_value_summary(unit, environment, key.span(), key.mode),
                    super::super::spec_methods::MethodEvidence::Deferred { summary }
                    | super::super::spec_methods::MethodEvidence::Contract { summary } => {
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
