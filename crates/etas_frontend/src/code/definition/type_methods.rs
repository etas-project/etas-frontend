use std::collections::HashMap;

use etas_code::descriptor::DescriptorLimits;
use etas_code::{ImplementationHeader, ImplementationKey, ImplementationSpecIdentity, MethodId};
use etas_hir::{HirImplTarget, HirItem, SymbolId};
use etas_types::Type;

use crate::code::{
    descriptor::{DefinitionDescriptorProjector, DescriptorInput, ProjectionError},
    identity::{DefinitionIdentityIndex, DefinitionOrigins},
};

pub(crate) fn type_method_identities(
    checked: DescriptorInput<'_>,
    identities: &DefinitionIdentityIndex,
    origins: &DefinitionOrigins,
    limits: DescriptorLimits,
) -> Result<HashMap<SymbolId, MethodId>, ProjectionError> {
    let mut implementations = HashMap::new();
    let mut methods = HashMap::new();
    for method in checked.types.type_impl_methods.values() {
        let implementation = match implementations.get(&method.implementation) {
            Some(identity) => identity,
            None => {
                let identity =
                    implementation_identity(checked, identities, origins, limits, method)?;
                implementations
                    .entry(method.implementation)
                    .or_insert(identity)
            }
        };
        methods.insert(
            method.symbol,
            MethodId::new(implementation.clone(), method.name.clone())?,
        );
    }
    Ok(methods)
}

fn implementation_identity(
    checked: DescriptorInput<'_>,
    identities: &DefinitionIdentityIndex,
    origins: &DefinitionOrigins,
    limits: DescriptorLimits,
    method: &etas_types::facts::TypeImplMethodFact,
) -> Result<ImplementationKey, ProjectionError> {
    let (constructor, arguments) = match checked.type_store.get(method.self_type) {
        Some(Type::Applied { constructor, args }) => {
            (etas_types::TypeId(constructor.0), args.as_slice())
        }
        Some(_) => (method.self_type, &[][..]),
        None => return Err(ProjectionError::MissingType(method.self_type)),
    };
    let subject = identities
        .types
        .get(&constructor)
        .ok_or(ProjectionError::MissingType(constructor))?
        .identity
        .clone();
    let mut projector = DefinitionDescriptorProjector::from_input(checked, identities, limits);
    let arguments =
        projector.project_implementation_pattern(&subject, &method.owner_params, arguments)?;
    let Some(HirItem::Impl(decl)) = checked.items.get(method.implementation) else {
        return Err(ProjectionError::Invalid(
            "type method has no checked implementation owner".into(),
        ));
    };
    let specs = match &decl.target {
        HirImplTarget::SpecSatisfaction { specs, .. } => specs.as_slice(),
        HirImplTarget::Inherent { .. } => &[],
        _ => {
            return Err(ProjectionError::Invalid(
                "type method implementation has the wrong target kind".into(),
            ));
        }
    };
    let mut selected = Vec::with_capacity(specs.len());
    for spec in specs {
        let etas_hir::ResolveResult::Resolved(symbol) = spec.spec_path.resolution else {
            return Err(ProjectionError::Invalid(
                "type implementation spec has no checked identity".into(),
            ));
        };
        let identity = identities
            .symbol(symbol)
            .ok_or(ProjectionError::MissingDeclaration(symbol))?
            .clone();
        let types = spec
            .spec_args
            .iter()
            .map(|ty| {
                checked
                    .types
                    .type_refs
                    .get(ty)
                    .copied()
                    .ok_or(ProjectionError::MissingTypeReference(*ty))
            })
            .collect::<Result<Vec<_>, _>>()?;
        selected.push((
            identity,
            projector.project_implementation_pattern(&subject, &method.owner_params, &types)?,
        ));
    }
    let data = checked
        .symbols
        .get(method.symbol)
        .ok_or(ProjectionError::MissingSymbol(method.symbol))?;
    let origin = origins
        .source_modules
        .get(&data.defining_module)
        .ok_or(ProjectionError::MissingPackage(data.defining_module))?;
    let graph = projector.finish()?;
    let arguments = arguments
        .into_iter()
        .map(|ty| graph.type_fingerprint(ty).map_err(ProjectionError::from))
        .collect::<Result<Vec<_>, _>>()?;
    let specs = selected
        .into_iter()
        .map(|(definition, arguments)| {
            Ok(ImplementationSpecIdentity {
                definition,
                arguments: arguments
                    .into_iter()
                    .map(|ty| graph.type_fingerprint(ty).map_err(ProjectionError::from))
                    .collect::<Result<_, _>>()?,
            })
        })
        .collect::<Result<Vec<_>, ProjectionError>>()?;
    let header = ImplementationHeader::new(specs)?.with_subject_arguments(arguments);
    Ok(ImplementationKey::from_header(
        origin.package(),
        origin.module().to_vec(),
        subject,
        None,
        &header,
    )?)
}
