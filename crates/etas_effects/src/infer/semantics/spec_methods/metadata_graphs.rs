use super::super::engine::EffectSemantics;
use crate::{
    ExternalActionTraceMetadata, ExternalCallableRequestTarget,
    ExternalRequestInstantiationMetadata, ExternalSpecMethodCallableArgument,
};
use etas_code::CodeFingerprint;
use etas_package_metadata::{CallableValueScope, GenericParamKind};
use etas_types::CallableGenericParamKind;
use std::{collections::BTreeMap, sync::Arc};

pub(crate) struct MethodCallbackGraph {
    method: Arc<crate::ExternalSpecMethodInvocation>,
    position: usize,
    pub generics: Vec<(String, CallableGenericParamKind)>,
    pub span: etas_core::Span,
}

impl MethodCallbackGraph {
    pub(crate) fn argument(&self) -> Option<&ExternalSpecMethodCallableArgument> {
        self.method.callable_arguments.get(self.position)
    }
}

type PendingMethod = (
    Arc<crate::ExternalSpecMethodInvocation>,
    Vec<(String, CallableGenericParamKind)>,
    etas_core::Span,
);

fn enqueue(
    pending: &mut Vec<PendingMethod>,
    targets: Vec<&ExternalCallableRequestTarget>,
    generics: &[(String, CallableGenericParamKind)],
    span: etas_core::Span,
) {
    pending.extend(targets.into_iter().filter_map(|target| match target {
        ExternalCallableRequestTarget::SpecMethod(method) => {
            Some((method.clone(), generics.to_vec(), span))
        }
        ExternalCallableRequestTarget::Parameter(_) => None,
    }));
}

pub(super) fn graph_key(method: CodeFingerprint, position: u32) -> CodeFingerprint {
    CodeFingerprint::of_parts(
        "etas.effects.method-callback.v1",
        [
            method.as_bytes().as_slice(),
            position.to_le_bytes().as_slice(),
        ],
    )
}

pub(super) fn targets<'a>(
    trace: &'a ExternalActionTraceMetadata,
    requests: Option<&'a ExternalRequestInstantiationMetadata>,
) -> Vec<&'a ExternalCallableRequestTarget> {
    let mut targets = requests
        .into_iter()
        .flat_map(|requests| &requests.parameters)
        .map(|parameter| &parameter.parameter)
        .collect::<Vec<_>>();
    let mut pending = vec![trace];
    while let Some(trace) = pending.pop() {
        match trace {
            ExternalActionTraceMetadata::ParameterCall { parameter } => targets.push(parameter),
            ExternalActionTraceMetadata::Widened {
                parameter_calls, ..
            } => targets.extend(parameter_calls),
            ExternalActionTraceMetadata::Seq(parts)
            | ExternalActionTraceMetadata::Choice(parts) => pending.extend(parts),
            ExternalActionTraceMetadata::Repeat(part) => pending.push(part),
            _ => {}
        }
    }
    targets
}

impl EffectSemantics<'_> {
    pub(crate) fn collect_method_callback_graphs(&mut self) {
        let mut index = BTreeMap::<_, Option<MethodCallbackGraph>>::new();
        let mut pending = Vec::new();
        for metadata in self.external_summaries {
            enqueue(
                &mut pending,
                targets(&metadata.action_trace, metadata.handler_requests.as_ref()),
                &metadata.generic_params,
                metadata.span,
            );
            if let Some(graph) = &metadata.returned_callables {
                for (position, node) in graph.values.iter().enumerate() {
                    if let Some(checked) = metadata.callable_values.get(position) {
                        let generics = node_generic_kinds(graph, node, &metadata.generic_params);
                        if let Some(generics) = generics {
                            enqueue(
                                &mut pending,
                                targets(&checked.action_trace, checked.handler_requests.as_ref()),
                                &generics,
                                metadata.span,
                            );
                        }
                    }
                }
            }
        }
        for definition in self.method_evidence.borrow().values() {
            pending.push((
                definition.invocation.clone(),
                definition.generics.clone(),
                definition.span,
            ));
        }
        let mut visited =
            BTreeMap::<CodeFingerprint, Vec<Vec<(String, CallableGenericParamKind)>>>::new();
        while let Some((method, generics, span)) = pending.pop() {
            let contexts = visited.entry(method.identity).or_default();
            if contexts.contains(&generics) {
                continue;
            }
            contexts.push(generics.clone());
            for (position, argument) in method.callable_arguments.iter().enumerate() {
                let key = graph_key(method.identity, argument.position);
                let entry = MethodCallbackGraph {
                    method: method.clone(),
                    position,
                    generics: generics.clone(),
                    span,
                };
                if let Some(existing) = index.get_mut(&key) {
                    if existing.as_ref().is_some_and(|existing| {
                        existing.argument() != entry.argument()
                            || existing.generics != entry.generics
                    }) {
                        *existing = None;
                    }
                } else {
                    index.insert(key, Some(entry));
                }
                for (position, node) in argument.sources.values.iter().enumerate() {
                    if let Some(checked) = argument.values.get(position)
                        && let Some(generics) =
                            node_generic_kinds(&argument.sources, node, &generics)
                    {
                        enqueue(
                            &mut pending,
                            targets(&checked.action_trace, checked.handler_requests.as_ref()),
                            &generics,
                            span,
                        );
                    }
                }
            }
        }
        self.method_callback_graphs = index;
    }

    pub(crate) fn metadata_evidence_generics(
        &self,
        source: crate::MetadataCallableSource,
    ) -> Option<Vec<(String, CallableGenericParamKind)>> {
        let (graph, node, _) = self.metadata_callable(source)?;
        let root = if self.implementation_graphs.contains_key(&source.graph) {
            Vec::new()
        } else if let Some(proof) = self.frozen_graphs.get(&source.graph) {
            self.native_generic_params(proof.identity.owner)?
                .iter()
                .map(|parameter| (parameter.name.clone(), parameter.kind))
                .collect()
        } else if let Some(entry) = self
            .method_callback_graphs
            .get(&source.graph)
            .and_then(Option::as_ref)
        {
            entry.generics.clone()
        } else {
            self.external_summaries
                .get(
                    self.metadata_callables
                        .get(&source.graph)
                        .copied()
                        .flatten()?,
                )?
                .generic_params
                .clone()
        };
        node_generic_kinds(graph, node, &root)
    }
}

fn node_generic_kinds(
    graph: &etas_package_metadata::CallableValueGraph,
    node: &etas_package_metadata::CallableValueNode,
    root: &[(String, CallableGenericParamKind)],
) -> Option<Vec<(String, CallableGenericParamKind)>> {
    node.generic_bindings
        .iter()
        .map(|binding| {
            let kind = match binding.scope {
                CallableValueScope::Root => root.get(binding.position as usize)?.1,
                CallableValueScope::Value(index) => match graph
                    .values
                    .get(index as usize)?
                    .generic_params
                    .get(binding.position as usize)?
                    .kind
                {
                    GenericParamKind::Type => CallableGenericParamKind::Type,
                    GenericParamKind::Effect => CallableGenericParamKind::Effect,
                },
            };
            Some((binding.label.clone(), kind))
        })
        .collect()
}
