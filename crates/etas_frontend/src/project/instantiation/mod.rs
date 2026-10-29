mod prepare;
mod recursive;
mod rows;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use etas_hir::HirItemId;
use etas_types::{CallableGenericParamKind, CallableSignature, EffectRowRef, TypeId};

pub(crate) use prepare::{InstantiationError, PreparationInput, PreparedInstantiations, prepare};
pub(crate) use rows::checked_row;

#[derive(Clone, Debug, PartialEq, Eq, Hash, etas_core::RetainedSize)]
pub(crate) enum NativeGenericArgument {
    Type(TypeId),
    EffectRow(EffectRowRef),
}

/// Binder identity is (owner, argument position), never a global parameter name.
#[derive(Clone, Debug, PartialEq, Eq, Hash, etas_core::RetainedSize)]
pub(crate) struct NativeInstantiation {
    pub owner: HirItemId,
    pub arguments: Vec<NativeGenericArgument>,
}

#[derive(Clone, Debug, Default, etas_core::RetainedSize)]
pub(crate) struct NativeInstantiationPlan {
    pub instances: Vec<NativeInstantiation>,
    pub recursive_templates: Vec<NativeInvocationTemplate>,
}

/// A checked recursive call is a template application, not a request to eagerly
/// enumerate every possible runtime argument in an unbounded monomorphization.
#[derive(Clone, Debug, PartialEq, Eq, etas_core::RetainedSize)]
pub(crate) struct NativeInvocationTemplate {
    pub owner: HirItemId,
    pub call: etas_hir::HirExprId,
    pub target: HirItemId,
}

pub(crate) type NativeBindings = (HashMap<String, TypeId>, HashMap<String, EffectRowRef>);

impl NativeInstantiation {
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
