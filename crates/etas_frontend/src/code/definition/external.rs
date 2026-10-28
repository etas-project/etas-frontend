use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use etas_code::{
    CallableId,
    descriptor::{CallableHeaderInfo, DescriptorGraph, DescriptorLimits},
};
use etas_types::{CheckedReflectionTarget, SymbolTypeFact};

use super::DefinitionCatalog;
use crate::code::descriptor::ProjectionError;

mod agents;
pub(super) use agents::ExternalAgent;

#[derive(Clone, PartialEq, Eq, etas_core::RetainedSize)]
pub(super) struct ExternalCallable {
    pub canonical: CallableId,
    pub header: CallableHeaderInfo,
    pub graph: DescriptorGraph,
    pub body: Option<Arc<etas_code::descriptor::CallableBodyInfo>>,
}

impl DefinitionCatalog {
    pub(super) fn bind_external_callables(
        &mut self,
        limits: DescriptorLimits,
    ) -> Result<(), ProjectionError> {
        let mut required = BTreeMap::new();
        for fact in self.checked.checked_reflections().values() {
            let CheckedReflectionTarget::Callable { declaration } = fact.target else {
                continue;
            };
            if self.callable_bindings.contains_key(&declaration) || fact.selected_spec.is_some() {
                continue;
            }
            let identity = self
                .identities
                .symbol(declaration)
                .ok_or(ProjectionError::MissingDeclaration(declaration))?;
            let callable = CallableId::declaration(identity.clone())?;
            if self.callables.contains_key(&callable) {
                self.callable_bindings.insert(declaration, callable);
            } else {
                required.insert(declaration, callable);
            }
        }
        let mut bindings = required.clone();
        for (symbol, fact) in &self.checked.types.symbol_types {
            if !matches!(fact, SymbolTypeFact::Flow { .. })
                || self.callable_bindings.contains_key(symbol)
            {
                continue;
            }
            if let Some(identity) = self.identities.symbol(*symbol)
                && matches!(
                    identity.kind(),
                    etas_code::DefinitionKind::Flow | etas_code::DefinitionKind::Tool
                )
            {
                let callable = CallableId::declaration(identity.clone())?;
                if self.callables.contains_key(&callable) {
                    self.callable_bindings.insert(*symbol, callable);
                } else {
                    bindings.insert(*symbol, callable);
                }
            }
        }
        let pending = bindings.values().collect::<BTreeSet<_>>();
        for metadata in self.checked.external_definition_origins() {
            let Some(set) = &metadata.descriptors else {
                continue;
            };
            let Some(headers) = &set.headers else {
                continue;
            };
            let selected = headers
                .callables
                .iter()
                .filter(|header| pending.contains(&header.id))
                .collect::<Vec<_>>();
            if selected.is_empty() {
                continue;
            }
            let graph = DescriptorGraph::new(set.graph.clone(), limits)?;
            let bodies = headers.bodies.as_ref().map(|bodies| {
                bodies
                    .entries
                    .iter()
                    .map(|body| (&body.callable, body))
                    .collect::<BTreeMap<_, _>>()
            });
            for header in selected {
                let body = bodies
                    .as_ref()
                    .and_then(|bodies| bodies.get(&header.id))
                    .map(|body| Arc::new((*body).clone()));
                if let Some(existing) = self.external_callables.get(&header.id) {
                    if existing.header != *header
                        || existing.graph != graph
                        || existing.body != body
                    {
                        return Err(ProjectionError::Invalid(
                            "external callable has conflicting portable definitions".into(),
                        ));
                    }
                } else {
                    self.external_callables.insert(
                        header.id.clone(),
                        ExternalCallable {
                            canonical: header.id.clone(),
                            header: header.clone(),
                            graph: graph.clone(),
                            body,
                        },
                    );
                }
            }
        }
        for (symbol, callable) in required {
            if !self.external_callables.contains_key(&callable) {
                return Err(ProjectionError::MissingIdentity(
                    "external reflected callable has no portable definition header".into(),
                ));
            }
            self.callable_bindings.insert(symbol, callable);
        }
        for (symbol, callable) in bindings {
            if self.external_callables.contains_key(&callable) {
                self.callable_bindings.insert(symbol, callable);
            }
        }
        Ok(())
    }
}
