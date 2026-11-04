use super::*;
use crate::facts::CapturedCallableAnalysis;
use crate::infer::semantics::specialize::EffectSpecializationError;
use crate::{
    MetadataCallableSource,
    facts::{MetadataCallableAnalysis, MetadataCallableGraphFact, MetadataCallableTypeFact},
};
use etas_package_metadata::{CallableValueScope, CallableValueSource};
use std::sync::Arc;

mod budget;

impl EffectSemantics<'_> {
    pub(crate) fn materialize_metadata_callable_analyses(
        &mut self,
    ) -> Result<(), EffectPipelineError> {
        for metadata in self.external_summaries {
            if let Some(control) = self.query_control {
                control
                    .checkpoint()
                    .map_err(EffectPipelineError::Interrupted)?;
            }
            let Some(contract) = &metadata.callable_contract else {
                continue;
            };
            match self
                .inputs
                .external_descriptor_graphs
                .entry(contract.descriptor_fingerprint)
            {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(contract.graph.clone());
                }
                std::collections::btree_map::Entry::Occupied(entry)
                    if entry.get() != &contract.graph =>
                {
                    return Err(invalid(
                        "external descriptor fingerprint identifies different graphs",
                    ));
                }
                std::collections::btree_map::Entry::Occupied(_) => {}
            }
        }
        let mut sources = self.discovered_external_method_sources.borrow().clone();
        let mut object_sources = BTreeSet::new();
        for witness in self
            .types
            .facts
            .existential_packs
            .values()
            .flat_map(|pack| &pack.witnesses)
        {
            let etas_types::method::MethodTarget::External { identity } = &witness.target else {
                continue;
            };
            let method = self
                .implementation_identities
                .get(identity)
                .and_then(|index| *index)
                .and_then(|index| self.external_implementations.get(index))
                .ok_or_else(|| {
                    invalid("object pack has no checked external implementation graph")
                })?;
            let [CallableValueSource::Value(value)] = method.graph.sources.as_slice() else {
                return Err(invalid("object implementation has no owned callable root"));
            };
            sources.insert(MetadataCallableSource {
                graph: method.graph_key,
                value: *value,
                environment: None,
                analysis: crate::CallableAnalysisKind::CheckedRevision,
            });
        }
        let contracts = self
            .types
            .facts
            .external_trace_spec_conformances
            .iter()
            .map(|fact| &fact.item)
            .chain(
                self.types
                    .facts
                    .external_callable_spec_satisfactions
                    .iter()
                    .map(|fact| &fact.item),
            );
        for target in contracts {
            let etas_package_metadata::CallableContractTarget::Method(identity) = target else {
                continue;
            };
            let mut definitions = self
                .external_implementations
                .iter()
                .filter(|method| &method.identity == identity);
            let (Some(method), None) = (definitions.next(), definitions.next()) else {
                return Err(invalid(
                    "method conformance has no unique anchored behavior graph",
                ));
            };
            let [CallableValueSource::Value(value)] = method.graph.sources.as_slice() else {
                return Err(invalid(
                    "method conformance has no owned callable graph root",
                ));
            };
            sources.insert(MetadataCallableSource {
                analysis: crate::CallableAnalysisKind::CheckedRevision,
                graph: method.graph_key,
                value: *value,
                environment: None,
            });
        }
        for key in self.callable_summaries.borrow().summaries.keys() {
            if let LatentSource::Metadata(source) = key.source {
                sources.insert(source);
            }
        }
        for origins in self
            .callable_environments
            .borrow()
            .values()
            .flat_map(|environment| environment.parameters.values())
            .chain(self.call_results.values.values())
        {
            for origin in origins {
                if let LatentSource::Metadata(source) = origin {
                    sources.insert(*source);
                } else if let LatentSource::MetadataObject(source) = origin {
                    object_sources.insert(*source);
                }
            }
        }
        for summary in self.inputs.unit_effects.values().chain(
            self.inputs
                .captured_callables
                .values()
                .filter_map(|analysis| match analysis {
                    CapturedCallableAnalysis::Solved(summary) => Some(summary.as_ref()),
                    _ => None,
                }),
        ) {
            if let Some(returned) = summary
                .exits
                .as_ref()
                .and_then(|exits| exits.callable_return_sources())
            {
                for source in returned {
                    if let CallableReturnSource::Metadata(source) = source {
                        sources.insert(source);
                    } else if let CallableReturnSource::MetadataObject(source) = source {
                        object_sources.insert(source);
                    }
                }
            }
        }
        let mut pending = sources
            .into_iter()
            .map(LatentSource::Metadata)
            .chain(object_sources.into_iter().map(LatentSource::MetadataObject))
            .collect::<Vec<_>>();
        let mut visited = BTreeSet::new();
        let mut graphs = std::collections::BTreeMap::new();
        let mut analyses = std::collections::BTreeMap::new();
        let mut nodes = 0;
        let mut bytes = 0;
        while let Some(origin) = pending.pop() {
            if !visited.insert(origin) {
                continue;
            }
            nodes += 1;
            let (key, environment) = match origin {
                LatentSource::Metadata(source) => (source.graph, source.environment),
                LatentSource::MetadataObject(source) => (source.graph, source.environment),
                _ => unreachable!("metadata graph traversal only queues metadata origins"),
            };
            if let Some(control) = self.query_control {
                control
                    .checkpoint()
                    .map_err(EffectPipelineError::Interrupted)?;
            }
            if nodes > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
                return Err(EffectPipelineError::AnalysisLimitExceeded {
                    resource: "metadata callable nodes",
                    limit: etas_package_metadata::MAX_METADATA_GRAPH_NODES,
                });
            }
            let (graph, values, objects, anchor) = self.callable_graph(key).ok_or_else(|| {
                invalid("callable graph has no unique checked proof and diagnostic anchor")
            })?;
            if let std::collections::btree_map::Entry::Vacant(entry) = graphs.entry(key) {
                budget::charge(graph, &mut nodes, &mut bytes)?;
                entry.insert(Arc::new(MetadataCallableGraphFact {
                    implementation: self
                        .implementation_graphs
                        .get(&key)
                        .and_then(|index| *index)
                        .and_then(|index| self.external_implementations.get(index))
                        .map(|method| method.identity.clone()),
                    graph: Arc::new(graph.clone()),
                    objects: objects.to_vec(),
                    signatures: values
                        .iter()
                        .map(|value| MetadataCallableTypeFact {
                            params: value.params.clone(),
                            output: value.output,
                        })
                        .collect(),
                }));
            }
            if let LatentSource::MetadataObject(source) = origin {
                let object = graph
                    .objects
                    .get(source.object as usize)
                    .ok_or_else(|| invalid("metadata object has no checked graph node"))?;
                let mut children = object.payload_sources.clone();
                children.extend(object.witnesses.iter().filter_map(
                    |witness| match witness.target {
                        etas_package_metadata::ExistentialWitnessTarget::Implementation {
                            value,
                            ..
                        } => Some(CallableValueSource::Value(value)),
                        etas_package_metadata::ExistentialWitnessTarget::Bound => None,
                    },
                ));
                if let CallableValueScope::Value(value) = object.parent_scope {
                    children.push(CallableValueSource::Value(value));
                }
                enqueue(&mut pending, &children, key, environment);
                continue;
            }
            let LatentSource::Metadata(source) = origin else {
                unreachable!()
            };
            let (_, node, _) = self
                .metadata_callable(source)
                .ok_or_else(|| invalid("metadata callable has no checked interface"))?;
            if let CallableValueScope::Value(value) = node.parent_scope {
                pending.push(LatentSource::Metadata(MetadataCallableSource {
                    value,
                    analysis: crate::CallableAnalysisKind::CheckedRevision,
                    ..source
                }));
            }
            enqueue(
                &mut pending,
                node.returned_sources.as_deref().unwrap_or(&[]),
                key,
                environment,
            );
            let analysis = match self.solve_callable_summary_in_mode(
                LatentSource::Metadata(source),
                anchor,
                super::super::CallableSummaryMode::Descriptor,
            ) {
                Ok(summary) => match self.metadata_environment_generics_in_mode(
                    source,
                    super::super::CallableSummaryMode::Descriptor,
                ) {
                    Ok((type_bindings, effect_row_bindings)) => MetadataCallableAnalysis::Solved {
                        summary: Arc::new(summary),
                        type_bindings,
                        effect_row_bindings,
                    },
                    Err(error) => MetadataCallableAnalysis::Unavailable {
                        reason: error.to_string(),
                    },
                },
                Err(EffectSpecializationError::Interrupted(interruption)) => {
                    return Err(EffectPipelineError::Interrupted(interruption));
                }
                Err(error) => MetadataCallableAnalysis::Unavailable {
                    reason: error.to_string(),
                },
            };
            analyses.insert(source, analysis);
        }
        self.inputs.metadata_callables = analyses;
        self.inputs.metadata_callable_graphs = graphs;
        Ok(())
    }
}

fn enqueue(
    pending: &mut Vec<LatentSource>,
    children: &[CallableValueSource],
    graph: etas_code::CodeFingerprint,
    environment: Option<crate::facts::CallableEnvironmentId>,
) {
    for child in children {
        match child {
            CallableValueSource::Existential(object) => {
                pending.push(LatentSource::MetadataObject(crate::MetadataObjectSource {
                    graph,
                    object: *object,
                    environment,
                }))
            }
            CallableValueSource::Value(value) | CallableValueSource::FrozenContract(value) => {
                pending.push(LatentSource::Metadata(MetadataCallableSource {
                    graph,
                    value: *value,
                    environment,
                    analysis: if matches!(child, CallableValueSource::FrozenContract(_)) {
                        crate::CallableAnalysisKind::FrozenContract
                    } else {
                        crate::CallableAnalysisKind::CheckedRevision
                    },
                }))
            }
            CallableValueSource::Parameter(_) => {}
        }
    }
}

fn invalid(reason: &str) -> EffectPipelineError {
    EffectPipelineError::InvalidCallableFacts {
        reason: reason.into(),
    }
}
