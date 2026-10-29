use etas_code::{
    CallableId,
    descriptor::{DescriptorGraph, TypeInfo, TypeInfoRef},
};
use etas_effects::{
    CallableReturnSource as Source,
    facts::{CallableGenericParameter as P, CallableGenericValue as V, CallableValueOrigin},
};
use etas_package_metadata::{CallableValueScope, CallableValueSource};
use etas_types::{CallableGenericParamKind, ItemSignature};

use crate::{CheckedProgram, code::descriptor::ProjectionError};

/// This proves origin and parameter preservation, not graph/shape equivalence.
/// A descriptive package graph, or an equal digest without compiler provenance,
/// cannot establish that the returned value pins the old executable revision.
pub(in crate::code::contract) fn preserves(
    preservation: &crate::code::contract::preservation::BodyPreservation<'_>,
    id: &CallableId,
    descriptor: &DescriptorGraph,
    signature: TypeInfoRef,
    candidate_owner: etas_hir::HirItemId,
    limits: etas_code::descriptor::DescriptorLimits,
) -> Result<bool, ProjectionError> {
    let frozen = preservation.frozen();
    let candidate = preservation.candidate();
    let Some(TypeInfo::Function(signature)) = descriptor.type_info(signature) else {
        return Ok(false);
    };
    if !matches!(
        descriptor.type_info(signature.output),
        Some(TypeInfo::Function(_))
    ) {
        // Aggregate payload positions need their own provenance; a union of
        // callable origins cannot prove that record/variant slots are unchanged.
        return Ok(false);
    }
    let Some(graph) = frozen.contract().latent_graph(id) else {
        return Ok(false);
    };
    let origins = &graph.origins;
    let graph = &graph.graph;
    let graph_fingerprint = etas_package_metadata::callable_value_graph_fingerprint(graph)
        .map_err(|error| invalid(&error.to_string()))?;
    let identity = etas_effects::FrozenCallableGraphIdentity {
        environment: frozen.fingerprint(),
        callable: id.clone(),
        graph: graph_fingerprint,
        owner: candidate_owner,
    };
    let key = identity.key();
    let Some(sources) = candidate
        .effects
        .item_effects
        .get(&candidate_owner)
        .and_then(|summary| summary.exits.as_ref())
        .and_then(|exits| exits.callable_return_sources())
    else {
        return Ok(false);
    };
    let params = parameters(&candidate.hir, candidate_owner)?;
    let references = sources
        .iter()
        .filter_map(|source| match source {
            Source::Reference(reference) => Some(reference),
            _ => None,
        })
        .collect::<Vec<_>>();
    if !references.is_empty()
        && !preserves_references(preservation, id, graph, origins, &references, limits)?
    {
        return Ok(false);
    }
    for source in sources {
        match source {
            Source::Reference(_) => {}
            Source::SourceItem(item) => {
                // Native graph origins are compiler evidence, not public
                // descriptor hashes. Only root values with the same retained
                // declaration slot can preserve an executable source origin.
                if !graph.sources.iter().any(|source| {
                    let CallableValueSource::Value(index) = source else {
                        return false;
                    };
                    matches!(
                        origins.get(*index as usize),
                        Some(crate::package_metadata::CallableGraphOrigin::Native {
                            unit: etas_effects::EffectUnit::Item(original),
                            environment: None,
                        }) if preservation.preserves_source_item(*original, item)
                    )
                }) {
                    return Ok(false);
                }
            }
            Source::Parameter(symbol) => {
                let Some(position) = params.iter().position(|param| *param == symbol) else {
                    return Ok(false);
                };
                if !graph.sources.contains(&CallableValueSource::Parameter(
                    etas_package_metadata::CallableValueParameter {
                        scope: CallableValueScope::Root,
                        position: position as u32,
                    },
                )) {
                    return Ok(false);
                }
            }
            Source::Metadata(source) => {
                if source.graph != key
                    || candidate.effects.frozen_callable_graphs.get(&key) != Some(&identity)
                    || !graph.sources.contains(&match source.analysis {
                        etas_effects::CallableAnalysisKind::CheckedRevision => {
                            CallableValueSource::Value(source.value)
                        }
                        etas_effects::CallableAnalysisKind::FrozenContract => {
                            CallableValueSource::FrozenContract(source.value)
                        }
                    })
                {
                    return Ok(false);
                }
                let Some(checked_graph) = candidate.effects.metadata_callable_graphs.get(&key)
                else {
                    return Ok(false);
                };
                if etas_package_metadata::callable_value_graph_fingerprint(&checked_graph.graph)
                    .map_err(|error| invalid(&error.to_string()))?
                    != graph_fingerprint
                {
                    return Ok(false);
                }
                if !preserves_bindings(candidate, candidate_owner, &params, source, graph)? {
                    return Ok(false);
                }
            }
            _ => return Ok(false),
        }
    }
    Ok(true)
}

fn preserves_references(
    preservation: &crate::code::contract::preservation::BodyPreservation<'_>,
    owner: &CallableId,
    graph: &etas_package_metadata::CallableValueGraph,
    origins: &[crate::package_metadata::CallableGraphOrigin],
    references: &[&etas_effects::CallableReturnReference],
    limits: etas_code::descriptor::DescriptorLimits,
) -> Result<bool, ProjectionError> {
    use crate::code::definition::ReferenceBindings;
    use etas_effects::{CallableReferenceTarget, facts::CallableReferenceAnalysis};
    let original = graph
        .sources
        .iter()
        .filter_map(|source| {
            let CallableValueSource::Value(index) = source else {
                return None;
            };
            match origins.get(*index as usize) {
                Some(crate::package_metadata::CallableGraphOrigin::Reference(reference))
                    if reference.environment.is_none() =>
                {
                    Some(reference)
                }
                _ => None,
            }
        })
        .collect::<Vec<_>>();
    if original.is_empty() {
        return Ok(false);
    }
    let relocated = preservation.relocate_reference_bindings(owner, &original, limits)?;
    for reference in references {
        let Some(binding) = ReferenceBindings::checked(reference) else {
            return Ok(false);
        };
        if !matches!(
            preservation
                .candidate()
                .effects
                .callable_references
                .get(*reference),
            Some(CallableReferenceAnalysis::Solved { .. })
        ) {
            return Ok(false);
        }
        if !original.iter().zip(&relocated).any(|(before, expected)| {
            match (&before.target, &reference.target) {
                (
                    CallableReferenceTarget::Source(before),
                    CallableReferenceTarget::Source(after),
                ) => preservation.preserves_source_item(*before, *after) && expected == &binding,
                _ => false,
            }
        }) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn preserves_bindings(
    candidate: &CheckedProgram,
    owner: etas_hir::HirItemId,
    params: &[etas_hir::SymbolId],
    source: etas_effects::MetadataCallableSource,
    graph: &etas_package_metadata::CallableValueGraph,
) -> Result<bool, ProjectionError> {
    let needed = graph
        .values
        .iter()
        .flat_map(|node| &node.parameter_bindings)
        .filter(|binding| binding.parameter.scope == CallableValueScope::Root)
        .map(|binding| binding.parameter)
        .collect::<std::collections::BTreeSet<_>>();
    let signature = match candidate.types.item_signatures.get(&owner) {
        Some(ItemSignature::Flow(signature) | ItemSignature::Tool(signature)) => signature,
        Some(ItemSignature::Agent(signature)) => signature
            .callable()
            .ok_or_else(|| invalid("frozen factory has no callable interface"))?,
        _ => return Err(invalid("frozen factory has no checked signature")),
    };
    let environment = source
        .environment
        .map(|id| {
            candidate
                .effects
                .callable_environments
                .get(&id)
                .ok_or_else(|| invalid("frozen callable origin has no checked environment"))
        })
        .transpose()?;
    if environment.is_some_and(|environment| environment.owner != owner) {
        return Ok(false);
    }
    for parameter in needed {
        let Some(symbol) = params.get(parameter.position as usize) else {
            return Ok(false);
        };
        let expected = [CallableValueOrigin::Parameter {
            owner,
            symbol: *symbol,
        }]
        .into();
        if environment.and_then(|environment| {
            environment
                .parameters
                .get(&etas_effects::ParameterCallTarget::Graph {
                    graph: source.graph,
                    parameter,
                })
        }) != Some(&expected)
        {
            return Ok(false);
        }
    }
    for (position, param) in signature.generic_params.iter().enumerate() {
        let key = P::Graph {
            graph: source.graph,
            scope: CallableValueScope::Root,
            position: position as u32,
        };
        let Some(actual) = environment.and_then(|environment| environment.generics.get(&key))
        else {
            return Ok(false);
        };
        match (param.kind, actual) {
            (CallableGenericParamKind::Type, V::Type(ty)) if *ty == param.subject => {}
            (CallableGenericParamKind::Effect, V::EffectRow(row))
                if row.effects.is_empty()
                    && row.open == Some(etas_effects::effect_var_id_from_name(&param.name)) => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

fn parameters(
    hir: &etas_hir::HirRevision,
    item: etas_hir::HirItemId,
) -> Result<Vec<etas_hir::SymbolId>, ProjectionError> {
    Ok(match hir.items.get(item) {
        Some(etas_hir::HirItem::Flow(flow)) => flow.params.clone(),
        Some(etas_hir::HirItem::Tool(tool)) => tool.params.clone(),
        Some(etas_hir::HirItem::Agent(agent)) => agent.params.clone(),
        _ => return Err(invalid("frozen factory parameter interface is missing")),
    })
}

fn invalid(reason: &str) -> ProjectionError {
    ProjectionError::Invalid(reason.into())
}

#[cfg(test)]
mod tests;
