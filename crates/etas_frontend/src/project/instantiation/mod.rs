mod index;
mod prepare;
mod rows;
mod targets;
#[cfg(test)]
mod tests;
mod witness;

use std::collections::HashMap;

use etas_hir::HirItemId;
use etas_types::{CallableGenericParamKind, CallableSignature, EffectRowRef, TypeId};

pub(crate) use index::normalized_row;
pub(crate) use prepare::{
    InstantiationError, InvocationInput, PreparationInput, PreparationPhase,
    PreparedInstantiations, prepare, prepare_from_targets,
};
pub(crate) use rows::checked_row;
pub(crate) use targets::direct_targets;

#[derive(
    Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub(crate) enum NativeGenericArgument {
    Type(TypeId),
    EffectRow(EffectRowRef),
}

/// Binder identity is (owner, argument position), never a global parameter name.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
pub(crate) struct NativeInstantiation {
    pub owner: HirItemId,
    pub arguments: Vec<NativeGenericArgument>,
    pub owner_bindings: Vec<(TypeId, TypeId)>,
}

#[derive(Clone, Debug, Default, etas_core::RetainedSize)]
pub(crate) struct NativeInstantiationPlan {
    instances: Vec<NativeInstantiation>,
    by_hash: HashMap<u64, Vec<usize>>,
}

impl NativeInstantiationPlan {
    pub fn instances(&self) -> &[NativeInstantiation] {
        &self.instances
    }
}

pub(crate) type NativeBindings = (HashMap<String, TypeId>, HashMap<String, EffectRowRef>);

impl NativeInstantiation {
    pub fn relocate_owner_bindings(
        &self,
        from: &[etas_types::CallableGenericParam],
        to: &[etas_types::CallableGenericParam],
        relocation: &mut etas_types::TypeSubstitutionEngine<'_>,
    ) -> Result<Vec<(TypeId, TypeId)>, InstantiationError> {
        if from.len() != to.len() || from.iter().zip(to).any(|(from, to)| from.kind != to.kind) {
            return Err("candidate invocation changed implementation binders".into());
        }
        self.owner_bindings
            .iter()
            .map(|(parameter, argument)| {
                let position = from
                    .iter()
                    .position(|declared| declared.subject == *parameter)
                    .ok_or("invocation owner binding has no checked declaration")?;
                Ok((to[position].subject, relocation.substitute(*argument)?))
            })
            .collect()
    }

    pub fn type_parameters(
        &self,
        signature: Option<&CallableSignature>,
    ) -> HashMap<TypeId, TypeId> {
        self.owner_bindings
            .iter()
            .copied()
            .chain(
                signature
                    .into_iter()
                    .flat_map(|signature| &signature.generic_params)
                    .zip(&self.arguments)
                    .filter_map(|(parameter, argument)| match argument {
                        NativeGenericArgument::Type(ty) => Some((parameter.subject, *ty)),
                        NativeGenericArgument::EffectRow(_) => None,
                    }),
            )
            .collect()
    }

    pub fn bindings(
        &self,
        signature: Option<&CallableSignature>,
    ) -> Result<NativeBindings, String> {
        let parameters = signature.map_or(&[][..], |signature| signature.generic_params.as_slice());
        if parameters.len() != self.arguments.len() {
            return Err("native instantiation has an inconsistent binder count".into());
        }
        let mut types = HashMap::new();
        let mut rows = HashMap::new();
        for (parameter, argument) in parameters.iter().zip(&self.arguments) {
            match (parameter.kind, argument) {
                (CallableGenericParamKind::Type, NativeGenericArgument::Type(ty)) => {
                    types.insert(parameter.name.clone(), *ty);
                }
                (CallableGenericParamKind::Effect, NativeGenericArgument::EffectRow(row)) => {
                    rows.insert(parameter.name.clone(), row.clone());
                }
                _ => return Err("native instantiation has an inconsistent binder kind".into()),
            }
        }
        Ok((types, rows))
    }
}

pub(crate) fn signature(
    facts: &etas_types::TypeFacts,
    item: HirItemId,
) -> Option<&CallableSignature> {
    match facts.item_signatures.get(&item)? {
        etas_types::ItemSignature::Flow(signature) | etas_types::ItemSignature::Tool(signature) => {
            Some(signature)
        }
        etas_types::ItemSignature::Agent(signature) => signature.callable(),
        etas_types::ItemSignature::TopLevelLet(_) => None,
    }
}

pub(crate) fn owner_parameters<'a>(
    hir: &etas_hir::HirProgram,
    facts: &'a etas_types::TypeFacts,
    item: HirItemId,
) -> &'a [etas_types::CallableGenericParam] {
    match hir.items.get(item) {
        Some(etas_hir::HirItem::Flow(flow)) => facts
            .type_impl_methods
            .get(&flow.symbol)
            .map_or(&[], |method| method.owner_params.as_slice()),
        _ => &[],
    }
}
