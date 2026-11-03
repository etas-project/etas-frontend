use std::collections::HashMap;

use crate::{
    CallableGenericParamKind, CallableSignature, TypeId, TypeInterner, TypeSubstitutionEngine,
};

pub(crate) fn specialize(
    interner: &mut TypeInterner,
    signature: &CallableSignature,
    receiver_param: Option<u32>,
    mut bindings: HashMap<String, TypeId>,
    receiver: TypeId,
) -> Result<CallableSignature, String> {
    let mut signature = signature.clone();
    if let Some(index) = receiver_param {
        let parameter = signature
            .generic_params
            .get(index as usize)
            .ok_or("spec receiver parameter index is out of bounds")?;
        if parameter.kind != CallableGenericParamKind::Type
            || signature.params.first() != Some(&parameter.subject)
        {
            return Err("spec receiver must bind the first callable input type".into());
        }
        bindings.insert(parameter.name.clone(), receiver);
        signature.generic_params.remove(index as usize);
    }
    let mut engine = TypeSubstitutionEngine::materializing(interner, &bindings, None);
    let mut apply = || -> Result<(), crate::TypeSubstitutionError> {
        for ty in &mut signature.params {
            *ty = engine.substitute(*ty)?;
        }
        signature.output = engine.substitute(signature.output)?;
        signature.effects = signature
            .effects
            .take()
            .map(|row| engine.substitute_effect_row(row))
            .transpose()?;
        signature.requested_actions = signature
            .requested_actions
            .take()
            .map(|row| engine.substitute_effect_row(row))
            .transpose()?;
        for parameter in &mut signature.generic_params {
            for bound in &mut parameter.bounds {
                for arg in &mut bound.args {
                    *arg = engine.substitute(*arg)?;
                }
            }
        }
        Ok(())
    };
    apply().map_err(|error| error.to_string())?;
    Ok(signature)
}
