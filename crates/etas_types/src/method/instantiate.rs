use std::collections::HashMap;

use crate::{
    CallableGenericParamKind, CallableSignature, TypeInterner, TypeSubstitutionEngine,
    facts::CheckedMethodCallFact,
};

use super::MethodSignatureFact;

pub(crate) fn signature(
    interner: &mut TypeInterner,
    declaration: &MethodSignatureFact,
    call: &CheckedMethodCallFact,
) -> Result<CallableSignature, String> {
    let mut parameters = call
        .owner_bindings
        .iter()
        .copied()
        .collect::<HashMap<_, _>>();
    let rows = call
        .instantiation
        .effect_row_bindings
        .iter()
        .cloned()
        .collect::<HashMap<_, _>>();
    for parameter in &declaration.signature.generic_params {
        match parameter.kind {
            CallableGenericParamKind::Type => {
                let ty = call
                    .instantiation
                    .type_bindings
                    .iter()
                    .find_map(|(name, ty)| (name == &parameter.name).then_some(*ty))
                    .ok_or_else(|| {
                        format!(
                            "checked method has no type argument for `{}`",
                            parameter.name
                        )
                    })?;
                parameters.insert(parameter.subject, ty);
            }
            CallableGenericParamKind::Effect if !rows.contains_key(&parameter.name) => {
                return Err(format!(
                    "checked method has no effect-row argument for `{}`",
                    parameter.name
                ));
            }
            CallableGenericParamKind::Effect => {}
        }
    }
    let names = HashMap::new();
    let mut engine = TypeSubstitutionEngine::materializing(interner, &names, None)
        .with_type_parameters(&parameters)
        .with_effect_arguments(&rows);
    let mut signature = declaration.signature.clone();
    if declaration.receiver == super::MethodReceiver::Component {
        signature.params.remove(0);
    }
    crate::facts::methods::map_signature(&mut signature, &mut |ty| engine.substitute(ty))
        .map_err(|error| error.to_string())?;
    for row in [&mut signature.effects, &mut signature.requested_actions]
        .into_iter()
        .flatten()
    {
        *row = engine
            .substitute_effect_row(row.clone())
            .map_err(|error| error.to_string())?;
    }
    Ok(signature)
}
