use std::collections::HashMap;

use etas_code::{
    ImplementationHeader, ImplementationKey, ImplementationSpecIdentity, MethodId,
    descriptor::{DescriptorLimits, GenericOwner, TypeInfoRef},
};
use etas_hir::{HirImplTarget, HirItem, HirItemId, ResolveResult, SymbolId};

use crate::code::{
    descriptor::{DefinitionDescriptorProjector, ProjectionError},
    identity::{DefinitionIdentityIndex, DefinitionOrigins},
};

struct PendingHeader {
    specs: Vec<(etas_code::DefinitionKey, Vec<TypeInfoRef>)>,
}

pub(crate) fn method_identities_from_input(
    checked: crate::code::descriptor::DescriptorInput<'_>,
    identities: &DefinitionIdentityIndex,
    origins: &DefinitionOrigins,
    limits: DescriptorLimits,
) -> Result<HashMap<SymbolId, MethodId>, ProjectionError> {
    let mut projector = DefinitionDescriptorProjector::from_input(checked, identities, limits);
    let mut headers = HashMap::<HirItemId, PendingHeader>::new();
    for method in checked.types.agent_methods.values() {
        let Some(implementation) = method.implementation else {
            continue;
        };
        if headers.contains_key(&implementation) {
            continue;
        }
        let Some(HirItem::Impl(decl)) = checked.items.get(implementation) else {
            return Err(ProjectionError::Invalid(
                "agent method has no checked implementation owner".into(),
            ));
        };
        let HirImplTarget::Agent { specs, .. } = &decl.target else {
            return Err(ProjectionError::Invalid(
                "agent method implementation has the wrong target kind".into(),
            ));
        };
        let agent_symbol = checked
            .items
            .get(method.agent)
            .and_then(HirItem::symbol)
            .ok_or(ProjectionError::MissingCallable(method.symbol))?;
        let subject = identities
            .symbol(agent_symbol)
            .ok_or(ProjectionError::MissingDeclaration(agent_symbol))?;
        let mut projected = Vec::with_capacity(specs.len());
        for spec in specs {
            let ResolveResult::Resolved(symbol) = spec.spec_path.resolution else {
                return Err(ProjectionError::Invalid(
                    "implementation spec has no resolved declaration identity".into(),
                ));
            };
            let identity = identities
                .symbol(symbol)
                .ok_or(ProjectionError::MissingDeclaration(symbol))?
                .clone();
            let args = spec
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
            let args = projector
                .project_closed_types(GenericOwner::Declaration(subject.clone()), &args)?;
            projected.push((identity, args));
        }
        headers.insert(implementation, PendingHeader { specs: projected });
    }
    let graph = projector.finish()?;
    let mut finished = HashMap::new();
    for (implementation, header) in headers {
        let specs = header
            .specs
            .into_iter()
            .map(|(definition, arguments)| {
                let arguments = arguments
                    .into_iter()
                    .map(|ty| graph.type_fingerprint(ty).map_err(ProjectionError::from))
                    .collect::<Result<_, _>>()?;
                Ok(ImplementationSpecIdentity {
                    definition,
                    arguments,
                })
            })
            .collect::<Result<Vec<_>, ProjectionError>>()?;
        finished.insert(implementation, ImplementationHeader::new(specs)?);
    }
    let implicit_header = ImplementationHeader::new(Vec::new())?;
    let mut methods = HashMap::new();
    for method in checked.types.agent_methods.values() {
        let header = match method.implementation {
            Some(implementation) => finished.get(&implementation).ok_or_else(|| {
                ProjectionError::Invalid("implementation header was not projected".into())
            })?,
            None => &implicit_header,
        };
        methods.insert(
            method.symbol,
            method_identity(checked, identities, origins, method, header)?,
        );
    }
    Ok(methods)
}

pub(crate) fn inherent_method_identities(
    checked: crate::code::descriptor::DescriptorInput<'_>,
    identities: &DefinitionIdentityIndex,
    origins: &DefinitionOrigins,
) -> Result<HashMap<SymbolId, MethodId>, ProjectionError> {
    let header = ImplementationHeader::new(Vec::new())?;
    checked
        .types
        .agent_methods
        .values()
        .filter(|method| method.inherent)
        .map(|method| {
            Ok((
                method.symbol,
                method_identity(checked, identities, origins, method, &header)?,
            ))
        })
        .collect()
}

fn method_identity(
    checked: crate::code::descriptor::DescriptorInput<'_>,
    identities: &DefinitionIdentityIndex,
    origins: &DefinitionOrigins,
    method: &etas_types::facts::signatures::AgentMethodSignature,
    header: &ImplementationHeader,
) -> Result<MethodId, ProjectionError> {
    let symbol = checked
        .symbols
        .get(method.symbol)
        .ok_or(ProjectionError::MissingSymbol(method.symbol))?;
    let agent = checked
        .items
        .get(method.agent)
        .and_then(HirItem::symbol)
        .and_then(|symbol| identities.symbol(symbol))
        .ok_or(ProjectionError::MissingCallable(method.symbol))?
        .clone();
    let origin = origins
        .source_modules
        .get(&symbol.defining_module)
        .ok_or(ProjectionError::MissingPackage(symbol.defining_module))?;
    let implementation = ImplementationKey::from_header(
        origin.package(),
        origin.module().to_vec(),
        agent,
        None,
        header,
    )?;
    Ok(MethodId::new(implementation, method.name.clone())?)
}

pub(super) fn callable_declarations(
    checked: crate::code::descriptor::DescriptorInput<'_>,
    identities: &DefinitionIdentityIndex,
) -> Result<Vec<(etas_code::CallableId, SymbolId, HirItemId)>, ProjectionError> {
    let methods = checked
        .types
        .agent_methods
        .keys()
        .map(|symbol| {
            identities
                .method(*symbol)
                .cloned()
                .map(|id| (*symbol, id))
                .ok_or(ProjectionError::MissingCallable(*symbol))
        })
        .collect::<Result<HashMap<_, _>, _>>()?;
    let mut pending = Vec::new();
    for (item, declaration) in checked.items.iter() {
        if !matches!(declaration, HirItem::Flow(_) | HirItem::Tool(_)) {
            continue;
        }
        let symbol = declaration
            .symbol()
            .ok_or_else(|| ProjectionError::Invalid("callable declaration has no symbol".into()))?;
        if methods.contains_key(&symbol) {
            continue;
        }
        let identity = identities
            .symbol(symbol)
            .ok_or(ProjectionError::MissingDeclaration(symbol))?
            .clone();
        pending.push((etas_code::CallableId::declaration(identity)?, symbol, item));
    }
    for (symbol, method) in methods {
        let body = checked
            .types
            .agent_methods
            .get(&symbol)
            .ok_or(ProjectionError::MissingCallable(symbol))?
            .body;
        pending.push((etas_code::CallableId::method(method), symbol, body));
    }
    pending.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(pending)
}
