use super::*;
use crate::code::{
    descriptor::{DefinitionDescriptorProjector, NativeDescriptorProjection},
    identity::DefinitionIdentityIndex,
};
use etas_code::{CallableId, DefinitionKey, descriptor::GenericOwner};
use std::collections::BTreeMap;

pub(super) struct Projection {
    pub identities: DefinitionIdentityIndex,
    pub native: NativeDescriptorProjection,
    pub callables: BTreeMap<CallableId, HirItemId>,
    pub values: BTreeMap<DefinitionKey, HirItemId>,
}

impl Projection {
    pub fn build(
        input: DescriptorInput<'_>,
        origins: &DefinitionOrigins,
        limits: etas_code::descriptor::DescriptorLimits,
    ) -> Result<Self, ProjectionError> {
        let identities = DefinitionIdentityIndex::from_input_with_limits(input, origins, limits)?;
        let declarations = super::super::headers::callable_declarations(input, &identities)?;
        let mut projector = DefinitionDescriptorProjector::from_input(input, &identities, limits);
        let mut named = identities
            .types
            .iter()
            .filter_map(|(ty, declaration)| declaration.standard.is_none().then_some(*ty))
            .chain(input.type_store.iter().filter_map(|(ty, value)| {
                matches!(value, etas_types::Type::Primitive(_)).then_some(ty)
            }))
            .collect::<Vec<_>>();
        named.sort_unstable();
        named.dedup();
        let mut callables = BTreeMap::new();
        for (id, symbol, item) in declarations {
            let signature = super::super::collect::callable_signature(input, item)
                .ok_or(ProjectionError::MissingCallable(symbol))?;
            let (names, declared) = super::super::collect::callable_source_header(input, item)?;
            let scope = owner(&id)?;
            let projected =
                projector.project_signature(scope.clone(), signature, &names, declared)?;
            projector.project_scoped_types(&scope, &named)?;
            if input.effects.is_some() {
                projector.project_analysis(&id, item, projected)?;
            }
            if callables.insert(id, item).is_some() {
                return Err(invalid(
                    "frozen callable has conflicting native declarations",
                ));
            }
        }
        let native = projector.finish_native()?;
        let mut values = BTreeMap::new();
        for (item, declaration) in input.hir.items.iter() {
            let etas_hir::HirItem::TopLevelLet(binding) = declaration else {
                continue;
            };
            let identity = identities
                .symbol(binding.symbol)
                .ok_or(ProjectionError::MissingDeclaration(binding.symbol))?;
            if values.insert(identity.clone(), item).is_some() {
                return Err(invalid("frozen value has conflicting native declarations"));
            }
        }
        Ok(Self {
            identities,
            native,
            callables,
            values,
        })
    }
}

pub(super) fn owner(id: &CallableId) -> Result<GenericOwner, ProjectionError> {
    match (id.as_method(), id.as_declaration()) {
        (Some(method), _) => Ok(GenericOwner::Method(method.clone())),
        (_, Some(declaration)) => Ok(GenericOwner::Declaration(declaration.clone())),
        _ => Err(invalid("frozen callable has no canonical generic owner")),
    }
}
