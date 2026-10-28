use super::{engine::EffectSemantics, shared::*, specialize::EffectSpecializationError};
use crate::{ActionRequestInstance, ParameterRequest, RequestInstantiationSummary};

impl EffectSemantics<'_> {
    pub(super) fn checked_request_instance(
        &self,
        expr: HirExprId,
        action: Effect,
        parameters: &[etas_types::CallableGenericParam],
        inputs: &[TypeId],
        output: TypeId,
    ) -> Result<RequestInstantiationSummary, EffectSpecializationError> {
        let mut bindings = Vec::new();
        for parameter in parameters
            .iter()
            .filter(|parameter| parameter.kind == etas_types::CallableGenericParamKind::Type)
        {
            let actual = self
                .types
                .facts
                .generic_instantiations
                .get(&expr)
                .and_then(|fact| {
                    fact.type_bindings
                        .iter()
                        .find(|(name, _)| name == &parameter.name)
                })
                .map(|(_, ty)| *ty);
            if let Some(actual) = actual {
                bindings.push((parameter.name.clone(), actual));
            } else {
                for ty in inputs.iter().copied().chain(std::iter::once(output)) {
                    if etas_types::type_contains_named_param(
                        &self.types.store,
                        ty,
                        &parameter.name,
                    )? {
                        return Err(EffectSpecializationError::MissingTypeBinding {
                            param: parameter.name.clone(),
                        });
                    }
                }
            }
        }
        Ok(RequestInstantiationSummary::action(action, bindings))
    }

    pub(super) fn specialize_request_types(
        &self,
        summary: &RequestInstantiationSummary,
        bindings: &[(String, TypeId)],
    ) -> Result<RequestInstantiationSummary, EffectSpecializationError> {
        let RequestInstantiationSummary::Available {
            actions,
            parameters,
        } = summary
        else {
            return Ok(RequestInstantiationSummary::Unavailable);
        };
        let bindings_map = bindings.iter().cloned().collect();
        let action = |action: &Effect| -> Result<Effect, EffectSpecializationError> {
            let row = self.specialize_effect_row(
                &EffectRow::closed(crate::EffectSet::one(action.clone())),
                &[],
                bindings,
                &[],
            )?;
            let mut effects = row.effects.iter();
            let action = effects
                .next()
                .cloned()
                .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?;
            if effects.next().is_some() || row.open.is_some() {
                return Err(EffectSpecializationError::InvalidCallableReturnSources);
            }
            Ok(action)
        };
        let actions = actions
            .iter()
            .map(|instance| {
                Ok(ActionRequestInstance {
                    action: action(&instance.action)?,
                    type_bindings: instance
                        .type_bindings
                        .as_ref()
                        .map(|bindings| {
                            bindings
                                .iter()
                                .map(|(name, ty)| {
                                    Ok((
                                        name.clone(),
                                        etas_types::substitute_named_params_in_store(
                                            &self.types.store,
                                            *ty,
                                            &bindings_map,
                                        )?,
                                    ))
                                })
                                .collect::<Result<_, EffectSpecializationError>>()
                        })
                        .transpose()?,
                })
            })
            .collect::<Result<_, EffectSpecializationError>>()?;
        let parameters = parameters
            .iter()
            .map(|parameter| {
                Ok(ParameterRequest {
                    target: parameter.target,
                    parameter: parameter.parameter.clone(),
                    span: parameter.span,
                    excluded: parameter
                        .excluded
                        .iter()
                        .map(action)
                        .collect::<Result<_, _>>()?,
                })
            })
            .collect::<Result<_, EffectSpecializationError>>()?;
        Ok(RequestInstantiationSummary::Available {
            actions,
            parameters,
        })
    }
}
