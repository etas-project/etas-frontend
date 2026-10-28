use super::*;
use crate::facts::{CallableEnvironment, CallableEnvironmentId};
use crate::infer::semantics::{specialize::TraceArgumentBindings, state::EffectState};

impl EffectSemantics<'_> {
    pub(crate) fn capture_callable_reference(
        &self,
        source: LatentSource,
        reference: &CallableReturnReference,
        application: HirExprId,
        bindings: &TraceArgumentBindings,
        state: &EffectState,
        invocation_environment: Option<CallableEnvironmentId>,
    ) -> Result<LatentSource, EffectSpecializationError> {
        let environment = if let Some(environment) = reference.environment {
            Some(self.instantiate_callable_environment(
                environment,
                application,
                bindings,
                state,
            )?)
        } else {
            let generics = self.native_creation_generics(
                reference.owner,
                application,
                state,
                invocation_environment,
            )?;
            if generics.is_empty() {
                None
            } else {
                let value = reference_value(source)
                    .ok_or_else(|| invalid("returned reference is not a checked function value"))?;
                let id = CallableEnvironmentId {
                    origin: self.callable_environment_site(value)?,
                    application: self.callable_environment_site(application)?,
                };
                self.record_callable_environment(
                    id,
                    CallableEnvironment {
                        owner: state.owner.ok_or_else(|| {
                            invalid("returned reference has no application owner")
                        })?,
                        parameters: Default::default(),
                        generics,
                    },
                )?;
                Some(id)
            }
        };
        with_environment(source, environment)
    }

    pub(crate) fn callable_reference_for_source(
        &self,
        source: LatentSource,
    ) -> Option<CallableReturnReference> {
        let (target, value, environment) = match source {
            LatentSource::Reference {
                unit: EffectUnit::Item(item),
                value,
                environment,
            } => (CallableReferenceTarget::Source(item), value, environment),
            LatentSource::StdReference { value, environment } => (
                CallableReferenceTarget::StdIntrinsic(
                    self.inputs.std_callable_values.get(&value)?.intrinsic,
                ),
                value,
                environment,
            ),
            _ => return None,
        };
        let mut reference = self.callable_reference_key(target, value)?;
        reference.environment = environment;
        Some(reference)
    }

    pub(in crate::infer::semantics) fn reference_environment_bindings(
        &self,
        reference: &CallableReturnReference,
    ) -> Result<super::super::callable_generics::GenericSubstitutions, EffectSpecializationError>
    {
        match reference.environment {
            Some(id) => self.native_environment_generics(
                reference.owner,
                &self.callable_environment(id)?.generics,
            ),
            None => Ok((vec![], vec![])),
        }
    }

    pub(crate) fn reference_invocation_environment(
        &self,
        source: LatentSource,
    ) -> Result<Option<CallableEnvironmentId>, EffectSpecializationError> {
        let reference = self
            .callable_reference_for_source(source)
            .ok_or_else(|| invalid("callable reference has no checked identity"))?;
        let CallableReferenceTarget::Source(owner) = reference.target else {
            return Ok(None);
        };
        let params = self
            .native_generic_params(owner)
            .ok_or_else(|| invalid("referenced callable has no generic interface"))?;
        if params.is_empty() {
            return Ok(None);
        }
        let (types, rows) = self.reference_environment_bindings(&reference)?;
        let substitutions = types.iter().cloned().collect();
        let application_owner = reference
            .environment
            .map(|id| self.callable_environment(id).map(|env| env.owner))
            .transpose()?
            .unwrap_or(reference.owner);
        let row_arguments = self.type_row_arguments(&rows, Some(application_owner))?;
        let mut generics = std::collections::BTreeMap::new();
        for (position, param) in params.iter().enumerate() {
            use crate::facts::{CallableGenericParameter as P, CallableGenericValue as V};
            let value = match param.kind {
                etas_types::CallableGenericParamKind::Type => {
                    let ty = reference
                        .type_bindings
                        .iter()
                        .find(|(name, _)| name == &param.name)
                        .ok_or_else(|| invalid("reference is missing its checked type argument"))?
                        .1;
                    V::Type(
                        etas_types::TypeSubstitutionEngine::existing(
                            &self.types.store,
                            &substitutions,
                        )
                        .with_effect_arguments(&row_arguments)
                        .substitute(ty)?,
                    )
                }
                etas_types::CallableGenericParamKind::Effect => {
                    let row = &reference
                        .effect_row_bindings
                        .iter()
                        .find(|(name, _)| name == &param.name)
                        .ok_or_else(|| invalid("reference is missing its checked effect argument"))?
                        .1;
                    V::EffectRow(self.specialize_effect_row(
                        &self.row_from_type_ref(row),
                        &[],
                        &types,
                        &rows,
                    )?)
                }
            };
            generics.insert(
                P::Native {
                    owner,
                    position: position as u32,
                },
                value,
            );
        }
        let value = reference_value(source)
            .ok_or_else(|| invalid("reference has no checked creation expression"))?;
        let site = self.callable_environment_site(value)?;
        let id = CallableEnvironmentId {
            origin: etas_code::CodeFingerprint::of_bytes(
                "etas.effects.reference-invocation.v1",
                site.as_bytes(),
            ),
            application: reference.environment.map_or(site, |id| id.application),
        };
        self.record_callable_environment(
            id,
            CallableEnvironment {
                owner: application_owner,
                parameters: Default::default(),
                generics,
            },
        )?;
        Ok(Some(id))
    }
}

pub(super) fn reference_value(source: LatentSource) -> Option<HirExprId> {
    match source {
        LatentSource::Reference { value, .. } | LatentSource::StdReference { value, .. } => {
            Some(value)
        }
        _ => None,
    }
}

pub(in crate::infer::semantics) fn with_environment(
    source: LatentSource,
    environment: Option<CallableEnvironmentId>,
) -> Result<LatentSource, EffectSpecializationError> {
    match source {
        LatentSource::Reference { unit, value, .. } => Ok(LatentSource::Reference {
            unit,
            value,
            environment,
        }),
        LatentSource::StdReference { value, .. } => {
            Ok(LatentSource::StdReference { value, environment })
        }
        _ => Err(invalid(
            "capture target is not a checked callable reference",
        )),
    }
}
