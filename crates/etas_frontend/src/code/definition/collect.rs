use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};

use etas_code::descriptor::{
    AgentHeaderInfo, CallableHeaderInfo, DefinitionDescriptorSet, DefinitionHeaders,
    DefinitionTypeBinding, DescriptorLimits, GenericOwner,
};
use etas_hir::{HirItem, HirItemId};
use etas_types::{CallableSignature, ItemSignature};

use super::store::{CatalogAgent, CatalogCallable, DefinitionCatalog, NativeCallableSlot};
use crate::{
    CheckedProgram,
    code::{
        descriptor::{AttachedSpecIndex, DefinitionDescriptorProjector, ProjectionError},
        identity::{DefinitionIdentityIndex, DefinitionOrigins},
    },
};

pub(super) fn build(
    checked: CheckedProgram,
    origins: &DefinitionOrigins,
    limits: DescriptorLimits,
) -> Result<DefinitionCatalog, ProjectionError> {
    let identities = DefinitionIdentityIndex::build_with_limits(&checked, origins, limits)?;
    build_selected(checked, origins, identities, limits, None).map(|(catalog, _)| catalog)
}

pub(super) fn build_exported(
    checked: CheckedProgram,
    origins: &DefinitionOrigins,
    identities: DefinitionIdentityIndex,
    limits: DescriptorLimits,
    selection: &BTreeSet<HirItemId>,
) -> Result<DefinitionDescriptorSet, ProjectionError> {
    let source = checked.clone();
    let (catalog, declarations) =
        build_selected(checked, origins, identities, limits, Some(selection))?;
    let agents = catalog
        .agents
        .values()
        .map(|agent| agent.header.clone())
        .collect::<Vec<_>>();
    let (snapshots, bodies) = super::snapshots::export(source, origins, limits, &agents)?;
    let set = DefinitionDescriptorSet {
        graph: catalog.graph.data().clone(),
        declarations,
        headers: Some(DefinitionHeaders {
            bodies: Some(bodies),
            snapshots: Some(snapshots),
            callables: catalog
                .callables
                .values()
                .map(|callable| callable.header.clone())
                .collect(),
            agents,
        }),
    };
    set.validate(limits)?;
    Ok(set)
}

fn build_selected(
    checked: CheckedProgram,
    origins: &DefinitionOrigins,
    identities: DefinitionIdentityIndex,
    limits: DescriptorLimits,
    selection: Option<&BTreeSet<HirItemId>>,
) -> Result<(DefinitionCatalog, Vec<DefinitionTypeBinding>), ProjectionError> {
    checked
        .reflection_targets
        .validate_owner(&checked.types)
        .map_err(ProjectionError::Invalid)?;
    let pending = super::headers::callable_declarations(checked.as_ref().into(), &identities)?;
    let mut projector = DefinitionDescriptorProjector::new(&checked, &identities, limits);
    let mut declarations = BTreeMap::new();
    if let Some(selection) = selection {
        let mut types = BTreeMap::new();
        for item in selection {
            let symbol = match checked.items.get(*item) {
                Some(HirItem::Type(decl)) => decl.symbol,
                Some(HirItem::TypeAlias(decl)) => decl.symbol,
                Some(HirItem::Enum(decl)) => decl.symbol,
                _ => continue,
            };
            let identity = identities
                .symbol(symbol)
                .ok_or(ProjectionError::MissingDeclaration(symbol))?
                .clone();
            types.insert(identity, symbol);
        }
        for (identity, symbol) in types {
            declarations.insert(identity, projector.project_type_declaration(symbol)?);
        }
    }
    let mut callables = BTreeMap::new();
    let mut callable_bindings = HashMap::new();
    for (id, symbol, item) in pending {
        if !selected_callable(&checked, &identities, symbol, item, selection)? {
            continue;
        }
        let signature = callable_signature(checked.as_ref().into(), item)
            .ok_or(ProjectionError::MissingCallable(symbol))?;
        let (names, declared) = callable_source_header(checked.as_ref().into(), item)?;
        let owner = if let Some(method) = id.as_method() {
            GenericOwner::Method(method.clone())
        } else {
            GenericOwner::Declaration(
                id.as_declaration()
                    .ok_or(ProjectionError::MissingCallable(symbol))?
                    .clone(),
            )
        };
        let projected = projector.project_signature(owner, signature, &names, declared)?;
        let analysis = projector.project_analysis(&id, item, projected)?;
        if selection.is_some()
            && let Some(identity) = id.as_declaration()
        {
            declarations.insert(identity.clone(), projected.ty);
        }
        let data = checked
            .symbols
            .get(symbol)
            .ok_or(ProjectionError::MissingSymbol(symbol))?;
        let record = CatalogCallable {
            header: CallableHeaderInfo {
                id: id.clone(),
                signature: projected.ty,
                analysis,
            },
            native: Arc::new(NativeCallableSlot {
                canonical: id.clone(),
                item,
                private_to: (data.visibility == etas_hir::Visibility::Private)
                    .then_some(data.defining_module),
            }),
        };
        if callables.insert(id.clone(), record).is_some() {
            return Err(ProjectionError::Invalid(
                "multiple callable bodies have the same stable declaration identity".into(),
            ));
        }
        callable_bindings.insert(symbol, id);
    }
    let mut agents = BTreeMap::new();
    let mut agent_bindings = HashMap::new();
    let annotation_bindings =
        super::annotations::AnnotationBindings::new(&checked, &identities, origins)?;
    let spec_index = AttachedSpecIndex::new(&checked.types);
    let method_spec_index =
        etas_types::facts::method_specs::AgentMethodSpecIndex::new(&checked.types);
    let method_signatures = checked
        .types
        .agent_methods
        .values()
        .filter_map(|method| callable_bindings.get(&method.symbol).map(|id| (id, method)))
        .collect::<BTreeMap<_, _>>();
    let mut pending_agents = BTreeMap::new();
    for (item, decl) in checked.items.iter() {
        let HirItem::Agent(decl) = decl else { continue };
        if selection.is_some_and(|selection| !selection.contains(&item)) {
            continue;
        }
        let identity = identities
            .symbol(decl.symbol)
            .ok_or(ProjectionError::MissingDeclaration(decl.symbol))?
            .clone();
        if pending_agents.insert(identity, (item, decl)).is_some() {
            return Err(ProjectionError::Invalid(
                "multiple agents have the same stable nominal identity".into(),
            ));
        }
    }
    for (identity, (item, decl)) in pending_agents {
        let Some(ItemSignature::Agent(signature)) = checked.types.item_signatures.get(&item) else {
            return Err(ProjectionError::MissingCallable(decl.symbol));
        };
        let default_method = match &signature.default_entry {
            Some(entry) => {
                let method = entry
                    .method
                    .ok_or(ProjectionError::MissingCallable(decl.symbol))?;
                if checked
                    .types
                    .agent_methods
                    .get(&method)
                    .is_none_or(|method| {
                        method.agent != item
                            || method.component != signature.component
                            || method.name != "run"
                            || method.callable != entry.callable
                            || method.callable_type != entry.callable_type
                    })
                {
                    return Err(ProjectionError::Invalid(
                        "default entry disagrees with its canonical agent method facts".into(),
                    ));
                }
                Some(method)
            }
            None => None,
        };
        let default_entry = default_method
            .map(|symbol| {
                callable_bindings
                    .get(&symbol)
                    .cloned()
                    .ok_or(ProjectionError::MissingCallable(symbol))
            })
            .transpose()?;
        let mut additional = checked
            .types
            .agent_methods
            .values()
            .filter(|method| method.agent == item && Some(method.symbol) != default_method)
            .filter(|method| selection.is_none() || method.private_to.is_none())
            .map(|method| {
                callable_bindings
                    .get(&method.symbol)
                    .cloned()
                    .ok_or(ProjectionError::MissingCallable(method.symbol))
            })
            .collect::<Result<Vec<_>, _>>()?;
        additional.sort();
        let data = checked
            .symbols
            .get(decl.symbol)
            .ok_or(ProjectionError::MissingSymbol(decl.symbol))?;
        let specs = projector.attached_specs(
            &spec_index,
            item,
            signature.component,
            &identity,
            default_entry.as_ref(),
        )?;
        let mut method_info = Vec::new();
        for id in default_entry.iter().chain(&additional) {
            let method = method_signatures.get(id).ok_or_else(|| {
                ProjectionError::MissingIdentity("agent method selection signature".into())
            })?;
            let mut specs = Vec::new();
            for binding in method_spec_index.bindings(method.symbol) {
                if checked
                    .types
                    .spec_signatures
                    .get(&binding.spec)
                    .is_none_or(|signature| signature.kind != binding.kind)
                {
                    return Err(ProjectionError::Invalid(
                        "method conformance has missing or inconsistent spec kind".into(),
                    ));
                }
                let definition = identities
                    .symbol(binding.spec)
                    .ok_or(ProjectionError::MissingDeclaration(binding.spec))?
                    .clone();
                let owner = GenericOwner::Method(
                    id.as_method()
                        .ok_or_else(|| {
                            ProjectionError::Invalid(
                                "agent method has no implementation identity".into(),
                            )
                        })?
                        .clone(),
                );
                let arguments = projector.project_scoped_types(&owner, binding.args)?;
                specs.push(etas_code::descriptor::SpecInfo {
                    definition,
                    arguments,
                });
            }
            specs.sort_by(|a, b| (&a.definition, &a.arguments).cmp(&(&b.definition, &b.arguments)));
            specs.dedup();
            method_info.push(etas_code::descriptor::AgentMethodInfo {
                callable: id.clone(),
                inherent: method.inherent,
                is_public: method.private_to.is_none(),
                specs,
            });
        }
        let header = AgentHeaderInfo {
            method_info: Some(method_info),
            identity: identity.clone(),
            component: projector.project_type_declaration(decl.symbol)?,
            default_entry,
            methods: additional,
            tools: super::tools::declared_tools(&checked, &identities, item)?,
            annotations: annotation_bindings.project(item, limits)?,
            specs,
        };
        if selection.is_some() {
            declarations.insert(identity.clone(), header.component);
        }
        if agents
            .insert(
                identity.clone(),
                CatalogAgent {
                    header,
                    private_to: (data.visibility == etas_hir::Visibility::Private)
                        .then_some(data.defining_module),
                    component: signature.component,
                },
            )
            .is_some()
        {
            return Err(ProjectionError::Invalid(
                "multiple agents have the same stable nominal identity".into(),
            ));
        }
        agent_bindings.insert(decl.symbol, identity);
    }
    let projection = projector.finish_native()?;
    let mut catalog = DefinitionCatalog {
        checked,
        origins: Arc::new(origins.clone()),
        identities,
        graph: projection.graph,
        type_origins: projection.type_origins,
        callables,
        external_callables: BTreeMap::new(),
        external_agents: BTreeMap::new(),
        agents,
        callable_bindings,
        agent_bindings,
        selected_bindings: HashMap::new(),
    };
    if selection.is_none() {
        catalog.bind_external_callables(limits)?;
        catalog.bind_external_agents(limits)?;
        catalog.add_selected_views()?;
    }
    Ok((
        catalog,
        declarations
            .into_iter()
            .map(|(definition, ty)| DefinitionTypeBinding { definition, ty })
            .collect(),
    ))
}

fn selected_callable(
    checked: &CheckedProgram,
    identities: &DefinitionIdentityIndex,
    symbol: etas_hir::SymbolId,
    item: HirItemId,
    selection: Option<&BTreeSet<HirItemId>>,
) -> Result<bool, ProjectionError> {
    let Some(selection) = selection else {
        return Ok(true);
    };
    if let Some(method) = checked.types.type_impl_methods.get(&symbol) {
        let id = identities
            .method(symbol)
            .ok_or(ProjectionError::MissingCallable(symbol))?;
        let exported = |definition: &etas_code::DefinitionKey| {
            selection.iter().any(|item| {
                checked
                    .items
                    .get(*item)
                    .and_then(HirItem::symbol)
                    .and_then(|symbol| identities.symbol(symbol))
                    == Some(definition)
            })
        };
        if !exported(id.implementation().subject()) {
            return Ok(false);
        }
        if method.inherent {
            return Ok(checked
                .symbols
                .get(symbol)
                .is_some_and(|symbol| symbol.visibility == etas_hir::Visibility::Public));
        }
        let Some(HirItem::Impl(decl)) = checked.items.get(method.implementation) else {
            return Err(ProjectionError::MissingCallable(symbol));
        };
        let etas_hir::HirImplTarget::SpecSatisfaction { specs, .. } = &decl.target else {
            return Err(ProjectionError::Invalid(
                "type method has no checked spec implementation".into(),
            ));
        };
        return Ok(specs.iter().any(|spec| match spec.spec_path.resolution {
            etas_hir::ResolveResult::Resolved(symbol) => {
                identities.symbol(symbol).is_some_and(exported)
            }
            _ => false,
        }));
    }
    let Some(method) = checked.types.agent_methods.get(&symbol) else {
        return Ok(selection.contains(&item));
    };
    if !selection.contains(&method.agent) {
        return Ok(false);
    }
    let Some(ItemSignature::Agent(signature)) = checked.types.item_signatures.get(&method.agent)
    else {
        return Err(ProjectionError::MissingCallable(symbol));
    };
    Ok(method.private_to.is_none()
        || signature
            .default_entry
            .as_ref()
            .is_some_and(|entry| entry.method == Some(symbol)))
}

pub(super) fn callable_signature<'a>(
    checked: crate::code::descriptor::DescriptorInput<'a>,
    item: HirItemId,
) -> Option<&'a CallableSignature> {
    match checked.types.item_signatures.get(&item)? {
        ItemSignature::Flow(signature) | ItemSignature::Tool(signature) => Some(signature),
        ItemSignature::Agent(signature) => signature.callable(),
        ItemSignature::TopLevelLet(_) => None,
    }
}

pub(super) fn callable_source_header(
    checked: crate::code::descriptor::DescriptorInput<'_>,
    item: HirItemId,
) -> Result<(Vec<Option<String>>, Option<etas_types::TypeId>), ProjectionError> {
    let (params, output) = match checked.items.get(item) {
        Some(HirItem::Flow(decl)) => (&decl.params, decl.return_type),
        Some(HirItem::Agent(decl)) => (&decl.params, decl.output_type),
        Some(HirItem::Tool(decl)) => (&decl.params, Some(decl.return_type)),
        _ => {
            return Err(ProjectionError::Invalid(
                "callable body has no source declaration header".into(),
            ));
        }
    };
    let names = params
        .iter()
        .map(|param| {
            let symbol = checked
                .symbols
                .get(*param)
                .ok_or(ProjectionError::MissingSymbol(*param))?;
            Ok((symbol.name != "_").then(|| symbol.name.clone()))
        })
        .collect::<Result<Vec<_>, ProjectionError>>()?;
    let output = output
        .map(|ty| {
            checked
                .types
                .type_refs
                .get(&ty)
                .copied()
                .ok_or(ProjectionError::MissingTypeReference(ty))
        })
        .transpose()?;
    Ok((names, output))
}
