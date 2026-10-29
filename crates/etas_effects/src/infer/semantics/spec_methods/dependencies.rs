use std::collections::{BTreeSet, HashSet};

use super::super::{
    engine::{EffectSemantics, external_summary_matches_path},
    shared::EffectUnit,
};
use super::metadata_graphs::targets;
use crate::ExternalCallableRequestTarget;
use etas_hir::{HirItem, SymbolDef};

impl EffectSemantics<'_> {
    pub(crate) fn external_method_dependencies(&self, path: &[String]) -> Vec<EffectUnit> {
        let mut methods = HashSet::new();
        for metadata in self
            .external_summaries
            .iter()
            .filter(|metadata| external_summary_matches_path(metadata, path))
        {
            let mut pending = targets(&metadata.action_trace, metadata.handler_requests.as_ref());
            for node in &metadata.callable_values {
                pending.extend(targets(&node.action_trace, node.handler_requests.as_ref()));
            }
            while let Some(target) = pending.pop() {
                if let ExternalCallableRequestTarget::SpecMethod(method) = target {
                    methods.insert((&method.spec, method.method.as_str()));
                    for argument in &method.callable_arguments {
                        for node in &argument.values {
                            pending.extend(targets(
                                &node.action_trace,
                                node.handler_requests.as_ref(),
                            ));
                        }
                    }
                }
            }
        }
        let mut dependencies = BTreeSet::new();
        for implementation in &self.types.facts.spec_impls {
            let Some(identity) = self
                .types
                .facts
                .external_specs
                .aliases
                .get(&implementation.spec_symbol)
            else {
                continue;
            };
            for method in &implementation.methods {
                if methods.contains(&(identity, method.name.as_str()))
                    && let Some(SymbolDef::Item { item }) = self
                        .hir
                        .symbols
                        .get(method.symbol)
                        .map(|symbol| &symbol.def)
                    && matches!(self.hir.items.get(*item), Some(HirItem::Flow(_)))
                {
                    dependencies.insert(EffectUnit::Item(*item));
                }
            }
        }
        dependencies.into_iter().collect()
    }
}
