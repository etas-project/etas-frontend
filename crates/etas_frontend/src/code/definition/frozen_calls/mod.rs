mod projection;
mod relocate;

use crate::code::{
    descriptor::{DescriptorInput, ProjectionError},
    identity::DefinitionOrigins,
};
use etas_effects::{
    EffectPipelineError, EffectRegistry, FrozenCallableEnvironment, FrozenCallableSummary,
};
use etas_hir::HirItemId;

#[derive(etas_core::RetainedSize)]
pub(crate) struct FrozenCalls<'a> {
    pub overlay: &'a crate::code::draft::DefaultBodyOverlay,
    pub hir: &'a etas_hir::HirRevision,
    pub types: &'a etas_types::TypeOutput,
    pub imports: &'a crate::ResolvedImports,
    pub modules: &'a crate::ModuleIndex,
    pub std_registry: &'a etas_std::StdRegistry,
    pub limits: etas_code::descriptor::DescriptorLimits,
}

impl etas_effects::FrozenCallableSummaryProvider for FrozenCalls<'_> {
    fn project(
        &self,
        registry: &EffectRegistry,
    ) -> Result<FrozenCallableEnvironment, EffectPipelineError> {
        self.project_inner(registry)
            .map_err(|error| match error.resource_limit() {
                Some((resource, limit)) => {
                    EffectPipelineError::AnalysisLimitExceeded { resource, limit }
                }
                None => EffectPipelineError::InvalidFrozenSummary {
                    reason: error.to_string(),
                },
            })
    }
}

impl FrozenCalls<'_> {
    fn project_inner(
        &self,
        registry: &EffectRegistry,
    ) -> Result<FrozenCallableEnvironment, ProjectionError> {
        let frozen = self.overlay.frozen();
        let original = frozen.checked();
        let source_origins = frozen.origins_for(original)?;
        let origins: DefinitionOrigins = frozen.origins_for_hir(self.hir)?;
        let input = DescriptorInput {
            hir: self.hir,
            types: &self.types.facts,
            type_store: &self.types.store,
            std_registry: self.std_registry,
            resolved_imports: self.imports,
            module_index: self.modules,
            effect_registry: registry,
            effects: None,
        };
        let from =
            projection::Projection::build(original.as_ref().into(), &source_origins, self.limits)?;
        let to = projection::Projection::build(input, &origins, self.limits)?;
        let relocation =
            relocate::Relocation::new(original, input, self.types, &from, &to, self.overlay)?;
        let mut output = FrozenCallableEnvironment::default();
        for (id, source) in &from.callables {
            let target = to.callables.get(id).copied().ok_or_else(|| {
                invalid("frozen callable is absent from the candidate declaration world")
            })?;
            let scope = projection::owner(id)?;
            let summary = match original.effects.item_effects.get(source) {
                Some(summary) => {
                    let returned = if summary
                        .exits
                        .as_ref()
                        .and_then(|exits| exits.callable_return_sources())
                        .is_some()
                    {
                        let graph = match frozen.contract().latent_graph(id) {
                            Some(graph) => Some(graph.clone()),
                            None => crate::package_metadata::checked_callable_graph(
                                original, *source, summary,
                            )
                            .map_err(|error| invalid(&error.to_string()))?
                            .map(std::sync::Arc::new),
                        };
                        let Some(proof) = graph else {
                            output
                                .callables
                                .insert(target, FrozenCallableSummary::Unavailable);
                            continue;
                        };
                        let graph = &proof.graph;
                        let graph_fingerprint =
                            etas_package_metadata::callable_value_graph_fingerprint(graph)
                                .map_err(|error| invalid(&error.to_string()))?;
                        let identity = etas_effects::FrozenCallableGraphIdentity {
                            environment: frozen.fingerprint(),
                            callable: id.clone(),
                            graph: graph_fingerprint,
                            owner: target,
                        };
                        let key = identity.key();
                        let source_types = etas_types::TypeOutput {
                            facts: original.types.clone(),
                            store: original.type_store.clone(),
                            diagnostics: vec![],
                        };
                        let signature =
                            super::collect::callable_signature(original.as_ref().into(), *source)
                                .ok_or_else(|| {
                                invalid("frozen callable graph has no checked generic scope")
                            })?;
                        let values = crate::project::lower_callable_graph_in_scope(
                            graph,
                            &source_types,
                            &signature.generic_params,
                        )
                        .map_err(|error| invalid(&error))?;
                        let values = relocation.graph_values(values, &scope)?;
                        let exits = relocation.graph_exits(&proof.origins, &scope)?;
                        let returned = relocation.graph_roots(graph, key, *source)?;
                        let anchor = original
                            .hir
                            .items
                            .get(*source)
                            .ok_or_else(|| {
                                invalid("frozen callable graph has no declaration anchor")
                            })?
                            .span();
                        output.graphs.insert(
                            key,
                            std::sync::Arc::new(etas_effects::FrozenCallableGraph {
                                identity,
                                graph: graph.clone(),
                                values,
                                exits,
                                anchor,
                            }),
                        );
                        Some(returned)
                    } else {
                        None
                    };
                    let environments = if returned.is_none() {
                        relocation.captured_environments(summary)?
                    } else {
                        std::collections::BTreeMap::new()
                    };
                    for (id, snapshot) in environments {
                        if output
                            .callable_environments
                            .insert(id, snapshot.clone())
                            .is_some_and(|previous| previous != snapshot)
                        {
                            return Err(invalid(
                                "frozen captures have conflicting relocated environments",
                            ));
                        }
                    }
                    FrozenCallableSummary::Solved(std::sync::Arc::new(relocation.summary(
                        summary,
                        &scope,
                        *source,
                        returned.as_ref(),
                    )?))
                }
                None if !original.reachability.analysis_items.contains(source)
                    && original.definition_sources.body(*source).is_some() =>
                {
                    FrozenCallableSummary::AnalyzePinnedSource
                }
                None => FrozenCallableSummary::Unavailable,
            };
            output.callables.insert(target, summary);
            let source_params = callable_params(&original.hir, *source)?;
            let target_params = callable_params(self.hir, target)?;
            if source_params.len() != target_params.len() {
                return Err(invalid("frozen callable parameter interface changed"));
            }
            for (source_param, target_param) in source_params.iter().zip(target_params) {
                if let Some(summary) = original
                    .effects
                    .inferred_parameter_effects
                    .get(source_param)
                {
                    for (id, snapshot) in relocation.captured_environments(summary)? {
                        if output
                            .callable_environments
                            .insert(id, snapshot.clone())
                            .is_some_and(|previous| previous != snapshot)
                        {
                            return Err(invalid(
                                "inferred callback captures have conflicting relocated environments",
                            ));
                        }
                    }
                    output.parameter_effects.insert(
                        *target_param,
                        relocation.summary(summary, &scope, *source, None)?,
                    );
                }
            }
        }
        output.method_evidence = relocation.into_method_evidence();
        Ok(output)
    }
}

fn callable_params(
    hir: &etas_hir::HirRevision,
    item: HirItemId,
) -> Result<&[etas_hir::SymbolId], ProjectionError> {
    match hir.items.get(item) {
        Some(etas_hir::HirItem::Flow(flow)) => Ok(&flow.params),
        Some(etas_hir::HirItem::Agent(agent)) => Ok(&agent.params),
        Some(etas_hir::HirItem::Tool(tool)) => Ok(&tool.params),
        _ => Err(invalid(
            "frozen callable has no checked parameter declaration",
        )),
    }
}

fn invalid(reason: &str) -> ProjectionError {
    ProjectionError::Invalid(reason.into())
}
