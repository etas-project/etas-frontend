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
        let instantiation = self.types.facts.generic_instantiations.get(&expr);
        let types = parameters
            .iter()
            .filter_map(|parameter| {
                bindings
                    .iter()
                    .find(|(name, _)| name == &parameter.name)
                    .map(|(_, ty)| (parameter.subject, *ty))
            })
            .collect();
        let rows = instantiation
            .into_iter()
            .flat_map(|fact| fact.effect_row_bindings.iter().cloned())
            .collect();
        let names = std::collections::HashMap::new();
        let sources = std::collections::HashMap::new();
        let mut engine = self
            .type_substitution(&names)
            .with_type_parameters(&types)
            .with_source_parameters(&sources)
            .with_effect_arguments(&rows);
        let mut summary = RequestInstantiationSummary::action(
            action,
            bindings,
            etas_types::ActionRequestSignature {
                params: inputs
                    .iter()
                    .map(|ty| engine.substitute(*ty))
                    .collect::<Result<_, _>>()?,
                output: engine.substitute(output)?,
            },
        );
        let injection = self.registry.action_by_name("std.effects.Inject.get");
        if let RequestInstantiationSummary::Available { actions, .. } = &mut summary {
            *actions = std::mem::take(actions)
                .into_iter()
                .map(|mut instance| {
                    let requested = match &instance.action {
                        Effect::Action(action) => Some(action),
                        Effect::AppliedAction(instance) => Some(&instance.action),
                        _ => None,
                    };
                    if requested == injection.as_ref() {
                        if let etas_hir::HirExpr::Perform { args, .. } = &self.hir.exprs[expr] {
                            if let [etas_hir::HirArg::Positional(value)] = args.as_slice() {
                                if let etas_hir::HirExpr::Literal(etas_hir::HirLiteral::String {
                                    value,
                                    ..
                                }) = &self.hir.exprs[*value]
                                {
                                    instance.injection_name = Some(value.clone());
                                }
                            }
                        }
                    }
                    instance
                })
                .collect();
        }
        Ok(summary)
    }
}
