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
        let mut sources = BTreeSet::new();
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
                    }
                }
            }
        }
        let mut pending = sources.iter().copied().collect::<Vec<_>>();
        let mut graphs = std::collections::BTreeMap::new();
        let mut analyses = std::collections::BTreeMap::new();
        let mut nodes = sources.len();
        let mut bytes = 0;
        while let Some(source) = pending.pop() {
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
            let (graph, values, anchor) = self.callable_graph(source.graph).ok_or_else(|| {
                invalid("callable graph has no unique checked proof and diagnostic anchor")
            })?;
            let (_, node, _) = self
                .metadata_callable(source)
                .ok_or_else(|| invalid("metadata callable has no checked interface"))?;
            if let std::collections::btree_map::Entry::Vacant(entry) = graphs.entry(source.graph) {
                budget::charge(graph, &mut nodes, &mut bytes)?;
                entry.insert(Arc::new(MetadataCallableGraphFact {
                    graph: Arc::new(graph.clone()),
                    signatures: values
                        .iter()
                        .map(|value| MetadataCallableTypeFact {
                            params: value.params.clone(),
                            output: value.output,
                        })
                        .collect(),
                }));
            }
            let mut children = node
                .returned_sources
                .iter()
                .flatten()
                .filter_map(|returned| match returned {
                    CallableValueSource::Value(value) => Some(*value),
                    _ => None,
                })
                .collect::<Vec<_>>();
            if let CallableValueScope::Value(value) = node.parent_scope {
                children.push(value);
            }
            for value in children {
                let child = MetadataCallableSource { value, ..source };
                if sources.insert(child) {
                    nodes += 1;
                    pending.push(child);
                }
            }
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

fn invalid(reason: &str) -> EffectPipelineError {
    EffectPipelineError::InvalidCallableFacts {
        reason: reason.into(),
    }
}
