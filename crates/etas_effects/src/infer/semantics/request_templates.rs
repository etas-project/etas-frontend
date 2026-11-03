use super::{engine::EffectSemantics, shared::*, specialize::EffectSpecializationError};
use crate::RequestInstantiationSummary;

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
        let substitutions = bindings.iter().cloned().collect();
        let lower = |ty| {
            etas_types::substitute_named_params_in_store(&self.types.store, ty, &substitutions)
        };
        Ok(RequestInstantiationSummary::action(
            action,
            bindings,
            etas_types::ActionRequestSignature {
                params: inputs
                    .iter()
                    .map(|ty| lower(*ty))
                    .collect::<Result<_, _>>()?,
                output: lower(output)?,
            },
        ))
    }
}
