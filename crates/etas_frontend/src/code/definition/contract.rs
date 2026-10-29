use std::{collections::BTreeMap, sync::Arc};

use etas_code::{
    CallableId, CodeFingerprint,
    descriptor::{AgentHeaderInfo, CallableHeaderInfo, DescriptorGraph, DeterminismInfo},
};

use super::DefinitionCatalog;
use crate::code::descriptor::ProjectionError;

mod application;
mod latent_fingerprint;

/// Immutable contract description. Native compilation additionally requires
/// the matching checked FrozenDefinitionEnvironment, not just these records.
#[derive(etas_core::RetainedSize)]
pub struct FrozenDefinitionContract {
    owner: FrozenContractOwner,
    graph: DescriptorGraph,
    callables: BTreeMap<CallableId, CallableHeaderInfo>,
    latent_graphs: BTreeMap<CallableId, Arc<crate::package_metadata::CheckedCallableGraph>>,
    request_instances: BTreeMap<CallableId, std::collections::BTreeSet<CodeFingerprint>>,
    applied: Option<crate::AppliedCallableBehavior>,
    fingerprint: CodeFingerprint,
}

#[derive(etas_core::RetainedSize)]
enum FrozenContractOwner {
    Agent(AgentHeaderInfo),
    Flow {
        identity: etas_code::DefinitionKey,
        entry: CallableId,
    },
}

impl FrozenContractOwner {
    fn identity(&self) -> &etas_code::DefinitionKey {
        match self {
            Self::Agent(header) => &header.identity,
            Self::Flow { identity, .. } => identity,
        }
    }
    fn entry(&self) -> Option<&CallableId> {
        match self {
            Self::Agent(header) => header.default_entry.as_ref(),
            Self::Flow { entry, .. } => Some(entry),
        }
    }
    fn methods(&self) -> &[CallableId] {
        match self {
            Self::Agent(header) => &header.methods,
            Self::Flow { .. } => &[],
        }
    }
}

impl FrozenDefinitionContract {
    pub(super) fn from_flow_description(
        header: CallableHeaderInfo,
        graph: DescriptorGraph,
        fingerprint: CodeFingerprint,
    ) -> Result<Arc<Self>, ProjectionError> {
        let identity = header
            .id
            .as_declaration()
            .filter(|identity| identity.kind() == etas_code::DefinitionKind::Flow)
            .ok_or_else(|| {
                ProjectionError::Invalid("flow description has no ordinary-flow identity".into())
            })?
            .clone();
        Ok(Arc::new(Self {
            owner: FrozenContractOwner::Flow {
                identity,
                entry: header.id.clone(),
            },
            graph,
            callables: BTreeMap::from([(header.id.clone(), header)]),
            latent_graphs: BTreeMap::new(),
            request_instances: BTreeMap::new(),
            applied: None,
            fingerprint,
        }))
    }

    pub(super) fn from_agent_description(
        header: AgentHeaderInfo,
        graph: DescriptorGraph,
        callables: BTreeMap<CallableId, CallableHeaderInfo>,
        fingerprint: CodeFingerprint,
    ) -> Arc<Self> {
        Arc::new(Self {
            latent_graphs: BTreeMap::new(),
            request_instances: BTreeMap::new(),
            applied: None,
            owner: FrozenContractOwner::Agent(header),
            graph,
            callables,
            fingerprint,
        })
    }
    pub(super) fn capture_agent(
        catalog: &DefinitionCatalog,
        header: AgentHeaderInfo,
    ) -> Result<Arc<Self>, ProjectionError> {
        Self::capture(catalog, FrozenContractOwner::Agent(header))
    }

    pub(super) fn capture_flow(
        catalog: &DefinitionCatalog,
        header: &CallableHeaderInfo,
    ) -> Result<Arc<Self>, ProjectionError> {
        let identity = header
            .id
            .as_declaration()
            .filter(|identity| identity.kind() == etas_code::DefinitionKind::Flow)
            .ok_or_else(|| {
                ProjectionError::Invalid(
                    "editable flow requires its own ordinary-flow declaration identity".into(),
                )
            })?;
        Self::capture(
            catalog,
            FrozenContractOwner::Flow {
                identity: identity.clone(),
                entry: header.id.clone(),
            },
        )
    }

    fn capture(
        catalog: &DefinitionCatalog,
        owner: FrozenContractOwner,
    ) -> Result<Arc<Self>, ProjectionError> {
        Self::capture_with_application(catalog, owner, None)
    }

    fn capture_with_application(
        catalog: &DefinitionCatalog,
        owner: FrozenContractOwner,
        applied: Option<application::AppliedContract>,
    ) -> Result<Arc<Self>, ProjectionError> {
        let mut callables = BTreeMap::new();
        for id in owner.entry().into_iter().chain(owner.methods()) {
            let callable = catalog.callables.get(id).ok_or_else(|| {
                ProjectionError::MissingIdentity("frozen callable contract".into())
            })?;
            let header = applied
                .as_ref()
                .map_or(&callable.header, |applied| &applied.header);
            if &header.id != id {
                return Err(ProjectionError::Invalid(
                    "applied contract has a different canonical callable identity".into(),
                ));
            }
            callables.insert(id.clone(), header.clone());
        }
        let graph = applied
            .as_ref()
            .map_or_else(|| catalog.graph.clone(), |applied| applied.graph.clone());
        let mut latent_graphs = BTreeMap::new();
        let mut request_instances = BTreeMap::new();
        for id in callables.keys() {
            let requests = match &applied {
                Some(applied) => {
                    request_instances_from_summary(catalog, id, applied.behavior.summary())?
                }
                None => callable_request_instances(catalog, id)?,
            };
            if let Some(instances) = requests {
                request_instances.insert(id.clone(), instances);
            }
            let item = catalog.item_owner(id).ok_or_else(|| {
                ProjectionError::MissingIdentity("frozen latent callable owner".into())
            })?;
            // Latent source keys belong to this pinned source program. Keep
            // their lexical graph paired with the sealed root application;
            // changing reference keys would sever the checked provenance.
            if let Some(summary) = catalog.checked.effects.item_effects.get(&item)
                && let Some(graph) =
                    crate::package_metadata::checked_callable_graph(&catalog.checked, item, summary)
                        .map_err(|error| ProjectionError::Invalid(error.to_string()))?
            {
                latent_graphs.insert(id.clone(), Arc::new(graph));
            }
        }
        let mut parts = match &owner {
            FrozenContractOwner::Agent(header) => {
                let mut selection = header
                    .method_info
                    .as_ref()
                    .ok_or_else(|| {
                        ProjectionError::MissingIdentity(
                            "native agent method selection facts".into(),
                        )
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
                vec![
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
                        None => {
                            CodeFingerprint::of_bytes("etas.code.no-default-contract-entry.v1", b"")
                        }
                    },
                ]
            }
            FrozenContractOwner::Flow { identity, entry } => {
                vec![identity.fingerprint(), entry.fingerprint()]
            }
        };
        if let Some(applied) = &applied {
            parts.push(applied.fingerprint);
        }
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
            fields.push(CodeFingerprint::of_bytes(
                "etas.code.contract-independent-determinism.v1",
                &[match callable.analysis.independent_determinism {
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
                "etas.code.frozen-callable-contract.v5",
                fields.iter().map(|part| part.as_bytes().as_slice()),
            ));
        }
        let fingerprint = CodeFingerprint::of_parts(
            "etas.code.frozen-definition-contract.v1",
            parts.iter().map(|part| part.as_bytes().as_slice()),
        );
        Ok(Arc::new(Self {
            owner,
            graph,
            callables,
            latent_graphs,
            request_instances,
            applied: applied.map(|applied| applied.behavior),
            fingerprint,
        }))
    }

    pub fn identity(&self) -> &etas_code::DefinitionKey {
        self.owner.identity()
    }
    pub fn entry(&self) -> Option<&CallableId> {
        self.owner.entry()
    }
    pub fn methods(&self) -> &[CallableId] {
        self.owner.methods()
    }
    pub fn agent_header(&self) -> Option<&AgentHeaderInfo> {
        match &self.owner {
            FrozenContractOwner::Agent(header) => Some(header),
            FrozenContractOwner::Flow { .. } => None,
        }
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
    pub fn applied_behavior(&self) -> Option<&crate::AppliedCallableBehavior> {
        self.applied.as_ref()
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
    request_instances_from_summary(catalog, id, summary)
}

fn request_instances_from_summary(
    catalog: &DefinitionCatalog,
    id: &CallableId,
    summary: &etas_effects::EffectSummary,
) -> Result<Option<std::collections::BTreeSet<CodeFingerprint>>, ProjectionError> {
    let item = catalog
        .item_owner(id)
        .ok_or_else(|| ProjectionError::MissingIdentity("applied request contract owner".into()))?;
    crate::package_metadata::checked_request_template(&catalog.checked, item, summary)
        .map_err(|error| ProjectionError::Invalid(error.to_string()))?
        .map(|template| latent_fingerprint::request_instances(catalog, &template))
        .transpose()
}
