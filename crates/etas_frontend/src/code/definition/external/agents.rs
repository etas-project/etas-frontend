use std::collections::BTreeMap;
use std::sync::Arc;

use etas_code::{
    CallableId,
    descriptor::{
        AgentHeaderInfo, CallableHeaderInfo, DefinitionSnapshotInfo, DescriptorGraph,
        DescriptorLimits,
    },
};
use etas_types::{CheckedReflectionTarget, SymbolTypeFact};

use super::{super::DefinitionCatalog, ExternalCallable};
use crate::code::descriptor::ProjectionError;

#[derive(Clone, PartialEq, Eq, etas_core::RetainedSize)]
pub(in crate::code::definition) struct ExternalAgent {
    pub header: AgentHeaderInfo,
    pub graph: DescriptorGraph,
    pub snapshot: Option<DefinitionSnapshotInfo>,
    pub callables: BTreeMap<CallableId, CallableHeaderInfo>,
    pub bodies: BTreeMap<CallableId, Arc<etas_code::descriptor::CallableBodyInfo>>,
    pub method_snapshots: BTreeMap<CallableId, DefinitionSnapshotInfo>,
    pub component: etas_types::TypeId,
}

impl DefinitionCatalog {
    pub(in crate::code::definition) fn bind_external_agents(
        &mut self,
        limits: DescriptorLimits,
    ) -> Result<(), ProjectionError> {
        let mut pending = BTreeMap::new();
        for (symbol, fact) in &self.checked.types.symbol_types {
            let SymbolTypeFact::Agent { signature } = fact else {
                continue;
            };
            let Some(identity) = self.identities.symbol(*symbol) else {
                continue;
            };
            if self.agents.contains_key(identity) {
                self.agent_bindings.insert(*symbol, identity.clone());
            } else {
                pending.insert(identity.clone(), signature.component);
            }
        }
        for metadata in self.checked.external_definition_origins() {
            let Some(set) = &metadata.descriptors else {
                continue;
            };
            let Some(headers) = &set.headers else {
                continue;
            };
            let selected = headers
                .agents
                .iter()
                .filter(|header| pending.contains_key(&header.identity))
                .collect::<Vec<_>>();
            if selected.is_empty() {
                continue;
            }
            let graph = DescriptorGraph::new(set.graph.clone(), limits)?;
            let snapshots = headers.snapshots.as_ref().map(|snapshots| {
                snapshots
                    .agents
                    .iter()
                    .map(|snapshot| (&snapshot.identity, snapshot))
                    .collect::<BTreeMap<_, _>>()
            });
            let callables = headers
                .callables
                .iter()
                .map(|header| (&header.id, header))
                .collect::<BTreeMap<_, _>>();
            let bodies = headers.bodies.as_ref().map(|bodies| {
                bodies
                    .entries
                    .iter()
                    .map(|body| (&body.callable, body))
                    .collect::<BTreeMap<_, _>>()
            });
            let method_snapshots = headers.snapshots.as_ref().map(|snapshots| {
                snapshots
                    .methods
                    .iter()
                    .map(|snapshot| (&snapshot.revision.definition, snapshot))
                    .collect::<BTreeMap<_, _>>()
            });
            for header in selected {
                let methods = header
                    .default_entry
                    .iter()
                    .chain(&header.methods)
                    .map(|id| {
                        let callable = callables.get(id).ok_or_else(|| {
                            invalid("external agent method has no portable header")
                        })?;
                        Ok((id.clone(), (*callable).clone()))
                    })
                    .collect::<Result<BTreeMap<_, _>, ProjectionError>>()?;
                let mut selected_bodies = BTreeMap::new();
                for (id, header) in &methods {
                    let body = bodies
                        .as_ref()
                        .and_then(|bodies| bodies.get(id))
                        .map(|body| Arc::new((*body).clone()));
                    if let Some(body) = &body {
                        selected_bodies.insert(id.clone(), body.clone());
                    }
                    let external = ExternalCallable {
                        canonical: header.id.clone(),
                        header: header.clone(),
                        graph: graph.clone(),
                        body,
                        snapshot: method_snapshots
                            .as_ref()
                            .and_then(|snapshots| snapshots.get(id))
                            .map(|snapshot| (*snapshot).clone()),
                    };
                    if let Some(existing) = self.external_callables.get(id)
                        && existing != &external
                    {
                        return Err(invalid("external agent method has conflicting definitions"));
                    }
                    self.external_callables.insert(id.clone(), external);
                }
                let selected_snapshots = methods
                    .keys()
                    .filter_map(|id| {
                        method_snapshots
                            .as_ref()?
                            .get(id)
                            .map(|snapshot| (id.clone(), (*snapshot).clone()))
                    })
                    .collect();
                let record = ExternalAgent {
                    header: header.clone(),
                    graph: graph.clone(),
                    snapshot: snapshots
                        .as_ref()
                        .and_then(|snapshots| snapshots.get(&header.identity))
                        .map(|snapshot| (*snapshot).clone()),
                    callables: methods,
                    bodies: selected_bodies,
                    method_snapshots: selected_snapshots,
                    component: pending[&header.identity],
                };
                if let Some(existing) = self.external_agents.get(&header.identity)
                    && existing != &record
                {
                    return Err(invalid(
                        "external agent has conflicting portable definitions",
                    ));
                }
                self.external_agents.insert(header.identity.clone(), record);
            }
        }
        for (symbol, fact) in &self.checked.types.symbol_types {
            if matches!(fact, SymbolTypeFact::Agent { .. })
                && let Some(identity) = self.identities.symbol(*symbol)
                && self.external_agents.contains_key(identity)
            {
                self.agent_bindings.insert(*symbol, identity.clone());
            }
        }
        for fact in self.checked.checked_reflections().values() {
            if let CheckedReflectionTarget::Agent { declaration, .. } = fact.target
                && !self.agent_bindings.contains_key(&declaration)
            {
                return Err(invalid(
                    "external reflected agent has no portable definition header",
                ));
            }
        }
        Ok(())
    }
}

fn invalid(message: &str) -> ProjectionError {
    ProjectionError::Invalid(message.into())
}
