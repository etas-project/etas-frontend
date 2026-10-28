use std::{collections::BTreeMap, sync::Arc};

use etas_code::{
    CallableId, CodeFingerprint,
    descriptor::{AgentHeaderInfo, CallableHeaderInfo, DescriptorGraph, DeterminismInfo},
};

use super::DefinitionCatalog;
use crate::code::descriptor::ProjectionError;

mod latent_fingerprint;

/// Immutable contract description. Native compilation additionally requires
/// the matching checked FrozenDefinitionEnvironment, not just these records.
#[derive(etas_core::RetainedSize)]
pub struct FrozenAgentContract {
    header: AgentHeaderInfo,
    graph: DescriptorGraph,
    callables: BTreeMap<CallableId, CallableHeaderInfo>,
    latent_graphs: BTreeMap<CallableId, Arc<crate::package_metadata::CheckedCallableGraph>>,
    request_instances: BTreeMap<CallableId, std::collections::BTreeSet<CodeFingerprint>>,
    fingerprint: CodeFingerprint,
}

impl FrozenAgentContract {
    pub(super) fn from_description(
        header: AgentHeaderInfo,
        graph: DescriptorGraph,
        callables: BTreeMap<CallableId, CallableHeaderInfo>,
        fingerprint: CodeFingerprint,
    ) -> Arc<Self> {
        Arc::new(Self {
            latent_graphs: BTreeMap::new(),
            request_instances: BTreeMap::new(),
            header,
            graph,
            callables,
            fingerprint,
        })
    }
    pub(super) fn capture(
        catalog: &DefinitionCatalog,
        header: AgentHeaderInfo,
    ) -> Result<Arc<Self>, ProjectionError> {
        let mut callables = BTreeMap::new();
        for id in header.default_entry.iter().chain(&header.methods) {
            let callable = catalog.callables.get(id).ok_or_else(|| {
                ProjectionError::MissingIdentity("frozen callable contract".into())
            })?;
            callables.insert(id.clone(), callable.header.clone());
        }
        let graph = catalog.graph.clone();
        let mut latent_graphs = BTreeMap::new();
        let mut request_instances = BTreeMap::new();
        for id in callables.keys() {
            if let Some(instances) = callable_request_instances(catalog, id)? {
                request_instances.insert(id.clone(), instances);
            }
            let item = catalog.item_owner(id).ok_or_else(|| {
                ProjectionError::MissingIdentity("frozen latent callable owner".into())
            })?;
            if let Some(summary) = catalog.checked.effects.item_effects.get(&item)
                && let Some(graph) =
                    crate::package_metadata::checked_callable_graph(&catalog.checked, item, summary)
                        .map_err(|error| ProjectionError::Invalid(error.to_string()))?
            {
                latent_graphs.insert(id.clone(), Arc::new(graph));
            }
        }
        let mut selection = header
            .method_info
            .as_ref()
            .ok_or_else(|| {
                ProjectionError::MissingIdentity("native agent method selection facts".into())
            })?
            .iter()
            .map(|method| method.fingerprint(&graph))
            .collect::<Result<Vec<_>, _>>()?;
        selection.sort_unstable();
        let specs = header
            .specs
            .iter()
            .map(|spec| spec.fingerprint(&graph))
            .collect::<Result<Vec<_>, _>>()?;
        let mut parts = vec![
            header.identity.fingerprint(),
            CodeFingerprint::of_parts(
                "etas.code.frozen-method-selection.v1",
                selection.iter().map(|part| part.as_bytes().as_slice()),
            ),
            graph.type_fingerprint(header.component)?,
            CodeFingerprint::of_parts(
                "etas.code.frozen-attached-specs.v1",
                specs.iter().map(|spec| spec.as_bytes().as_slice()),
            ),
            CodeFingerprint::of_parts(
                "etas.code.frozen-annotations.v1",
                header
                    .annotations
                    .iter()
                    .map(|annotation| annotation.fingerprint())
                    .collect::<Vec<_>>()
                    .iter()
                    .map(|fingerprint| fingerprint.as_bytes().as_slice()),
            ),
            CodeFingerprint::of_parts(
                "etas.code.frozen-tool-surface.v1",
                header
                    .tools
                    .iter()
                    .map(|tool| tool.fingerprint())
                    .collect::<Vec<_>>()
                    .iter()
                    .map(|fingerprint| fingerprint.as_bytes().as_slice()),
            ),
            match &header.default_entry {
                Some(callable) => CodeFingerprint::of_parts(
                    "etas.code.default-contract-entry.v1",
                    [callable.fingerprint().as_bytes().as_slice()],
                ),
                None => CodeFingerprint::of_bytes("etas.code.no-default-contract-entry.v1", b""),
            },
        ];
        for callable in callables.values() {
            let signature = graph.type_fingerprint(callable.signature)?;
            let rows = &callable.analysis.effects;
            let mut fields = vec![callable.id.fingerprint(), signature];
            fields.push(match latent_graphs.get(&callable.id) {
                Some(graph) => latent_fingerprint::fingerprint(catalog, &graph.graph)?,
                None => CodeFingerprint::of_bytes("etas.code.no-latent-contract-proof.v1", b""),
            });
            fields.push(match request_instances.get(&callable.id) {
                Some(instances) => CodeFingerprint::of_parts(
                    "etas.code.frozen-request-instances.v1",
                    instances.iter().map(|value| value.as_bytes().as_slice()),
                ),
                None => {
                    CodeFingerprint::of_bytes("etas.code.unavailable-request-instances.v1", b"")
                }
            });
            for row in [
                rows.declared_bound,
                rows.escaping,
                rows.requested,
                rows.default_handled,
            ] {
                fields.push(match row {
                    Some(row) => CodeFingerprint::of_parts(
                        "etas.code.present-contract-row.v1",
                        [graph.row_fingerprint(row)?.as_bytes().as_slice()],
                    ),
                    None => CodeFingerprint::of_bytes("etas.code.absent-contract-row.v1", b""),
                });
            }
            fields.push(CodeFingerprint::of_bytes(
                "etas.code.contract-determinism.v1",
                &[match callable.analysis.determinism {
                    None => 0,
                    Some(DeterminismInfo::Deterministic) => 1,
                    Some(DeterminismInfo::RuntimeMediated) => 2,
                    Some(DeterminismInfo::NonDeterministic) => 3,
                }],
            ));
            fields.push(match callable.analysis.trace {
                Some(trace) => CodeFingerprint::of_parts(
                    "etas.code.present-contract-trace.v1",
                    [graph.trace_fingerprint(trace)?.as_bytes().as_slice()],
                ),
                None => CodeFingerprint::of_bytes("etas.code.absent-contract-trace.v1", b""),
            });
            fields.push(match callable.analysis.outcomes {
                Some(trace) => CodeFingerprint::of_parts(
                    "etas.code.present-contract-outcomes.v1",
                    [graph.trace_fingerprint(trace)?.as_bytes().as_slice()],
                ),
                None => CodeFingerprint::of_bytes("etas.code.absent-contract-outcomes.v1", b""),
            });
            parts.push(CodeFingerprint::of_parts(
                "etas.code.frozen-callable-contract.v4",
                fields.iter().map(|part| part.as_bytes().as_slice()),
            ));
        }
        let fingerprint = CodeFingerprint::of_parts(
            "etas.code.frozen-agent-contract.v4",
            parts.iter().map(|part| part.as_bytes().as_slice()),
        );
        Ok(Arc::new(Self {
            header,
            graph,
            callables,
            latent_graphs,
            request_instances,
            fingerprint,
        }))
    }

    pub fn header(&self) -> &AgentHeaderInfo {
        &self.header
    }
    pub fn descriptors(&self) -> &DescriptorGraph {
        &self.graph
    }
    pub fn callables(&self) -> impl Iterator<Item = (&CallableId, &CallableHeaderInfo)> {
        self.callables.iter()
    }
    pub fn callable(&self, id: &CallableId) -> Option<&CallableHeaderInfo> {
        self.callables.get(id)
    }
    pub fn fingerprint(&self) -> CodeFingerprint {
        self.fingerprint
    }
    pub(crate) fn request_instances(
        &self,
        id: &CallableId,
    ) -> Option<&std::collections::BTreeSet<CodeFingerprint>> {
        self.request_instances.get(id)
    }
    pub(crate) fn latent_graph(
        &self,
        id: &CallableId,
    ) -> Option<&Arc<crate::package_metadata::CheckedCallableGraph>> {
        self.latent_graphs.get(id)
    }
}

pub(crate) fn callable_request_instances(
    catalog: &DefinitionCatalog,
    id: &CallableId,
) -> Result<Option<std::collections::BTreeSet<CodeFingerprint>>, ProjectionError> {
    let item = catalog.item_owner(id).ok_or_else(|| {
        ProjectionError::MissingIdentity("request contract callable owner".into())
    })?;
    let Some(summary) = catalog.checked.effects.item_effects.get(&item) else {
        return Ok(None);
    };
    crate::package_metadata::checked_request_template(&catalog.checked, item, summary)
        .map_err(|error| ProjectionError::Invalid(error.to_string()))?
        .map(|template| latent_fingerprint::request_instances(catalog, &template))
        .transpose()
}
