use crate::{
    CallableSignature, GenericInstantiationFact, TypeId,
    method::{MethodEvidence, MethodReceiver, MethodTarget},
};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct CheckedMethodCallFact {
    pub target: MethodTarget,
    pub receiver: MethodReceiver,
    pub receiver_type: TypeId,
    pub signature: CallableSignature,
    pub argument_slots: Vec<u32>,
    pub owner_bindings: Vec<(TypeId, TypeId)>,
    pub instantiation: GenericInstantiationFact,
    pub evidence: MethodEvidence,
}

impl CheckedMethodCallFact {
    pub(crate) fn try_map_types<E>(
        &mut self,
        mut map: impl FnMut(TypeId) -> Result<TypeId, E>,
    ) -> Result<(), E> {
        self.receiver_type = map(self.receiver_type)?;
        map_signature(&mut self.signature, &mut map)?;
        for (parameter, argument) in &mut self.owner_bindings {
            *parameter = map(*parameter)?;
            *argument = map(*argument)?;
        }
        match &mut self.evidence {
            MethodEvidence::Inherent => {}
            MethodEvidence::Implementation { subject, args, .. }
            | MethodEvidence::Bound { subject, args, .. } => {
                *subject = map(*subject)?;
                for arg in args {
                    *arg = map(*arg)?;
                }
            }
        }
        for (_, ty) in &mut self.instantiation.type_bindings {
            *ty = map(*ty)?;
        }
        for (_, row) in &mut self.instantiation.effect_row_bindings {
            map_row(row, &mut map)?;
        }
        for obligation in &mut self.instantiation.deferred_effect_row_obligations {
            map_row(&mut obligation.fixed, &mut map)?;
            for equation in &mut obligation.concrete {
                map_row(&mut equation.fixed, &mut map)?;
                map_row(&mut equation.actual, &mut map)?;
            }
        }
        Ok(())
    }
}

pub(crate) fn map_signature<E>(
    signature: &mut CallableSignature,
    map: &mut impl FnMut(TypeId) -> Result<TypeId, E>,
) -> Result<(), E> {
    for ty in &mut signature.params {
        *ty = map(*ty)?;
    }
    signature.output = map(signature.output)?;
    map_parameters(&mut signature.generic_params, map)?;
    for row in [&mut signature.effects, &mut signature.requested_actions]
        .into_iter()
        .flatten()
    {
        map_row(row, map)?;
    }
    Ok(())
}

pub(crate) fn map_parameters<E>(
    parameters: &mut [crate::CallableGenericParam],
    map: &mut impl FnMut(TypeId) -> Result<TypeId, E>,
) -> Result<(), E> {
    for parameter in parameters {
        parameter.subject = map(parameter.subject)?;
        for bound in &mut parameter.bounds {
            for arg in &mut bound.args {
                *arg = map(*arg)?;
            }
        }
    }
    Ok(())
}

fn map_row<E>(
    row: &mut crate::EffectRowRef,
    map: &mut impl FnMut(TypeId) -> Result<TypeId, E>,
) -> Result<(), E> {
    for effect in &mut row.effects {
        for arg in &mut effect.args {
            if let crate::EffectArgRef::Type(ty) = arg {
                *ty = map(*ty)?;
            }
        }
    }
    Ok(())
}
