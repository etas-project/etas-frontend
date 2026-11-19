use std::collections::BTreeSet;

use super::specialize::{EffectSpecializationError, TraceArgumentBindings};
use super::{engine::EffectSemantics, shared::*};

impl EffectSemantics<'_> {
    pub(crate) fn returned_callable_bindings(
        &self,
        returned: &BTreeSet<crate::CallableReturnSource>,
        bindings: &TraceArgumentBindings,
        state: &EffectState,
        application: HirExprId,
        invocation_environment: Option<crate::facts::CallableEnvironmentId>,
    ) -> Result<Vec<LatentSource>, EffectSpecializationError> {
        let mut sources = BTreeSet::new();
        for source in returned {
            let parameter = match source {
                crate::CallableReturnSource::Projection(id) => {
                    let binding = self.trace_argument(
                        crate::ParameterCallTarget::Projection(*id),
                        "returned value projection",
                        bindings,
                        state,
                    )?;
                    if let Some(binding) = binding {
                        sources.extend(self.trace_argument_origins_at(
                            &binding,
                            state,
                            application,
                        )?);
                    } else {
                        sources.insert(LatentSource::Projection(*id));
                    }
                    continue;
                }

                crate::CallableReturnSource::MetadataObject(source) => {
                    let mut source = *source;
                    if let Some(environment) = source.environment {
                        source.environment = Some(self.instantiate_callable_environment(
                            environment,
                            application,
                            bindings,
                            state,
                        )?);
                    } else if self.frozen_graphs.contains_key(&source.graph) {
                        source.environment = self.bind_frozen_graph_environment(
                            source.graph,
                            application,
                            bindings,
                            state,
                            invocation_environment,
                        )?;
                    }
                    sources.insert(LatentSource::MetadataObject(source));
                    continue;
                }
                crate::CallableReturnSource::Existential { pack, environment } => {
                    let environment = self.instantiate_callable_environment(
                        *environment,
                        application,
                        bindings,
                        state,
                    )?;
                    sources.insert(LatentSource::Existential {
                        pack: *pack,
                        environment,
                    });
                    continue;
                }
                crate::CallableReturnSource::ExternalFrozen { reflection } => {
                    self.external_reflection_metadata(*reflection)
                        .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?;
                    sources.insert(LatentSource::ExternalFrozen {
                        reflection: *reflection,
                    });
                    continue;
                }
                crate::CallableReturnSource::ExternalReference { value, frozen } => {
                    self.external_reference_metadata(*value)
                        .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?;
                    sources.insert(LatentSource::ExternalReference {
                        value: *value,
                        frozen: *frozen,
                    });
                    continue;
                }
                crate::CallableReturnSource::FrozenContract {
                    unit,
                    reference,
                    environment,
                } => {
                    let environment = environment
                        .map(|environment| {
                            self.instantiate_callable_environment(
                                environment,
                                application,
                                bindings,
                                state,
                            )
                        })
                        .transpose()?;
                    sources.insert(LatentSource::FrozenContract {
                        unit: *unit,
                        reference: *reference,
                        environment,
                    });
                    continue;
                }
                crate::CallableReturnSource::Captured { unit, environment } => {
                    let environment = self.instantiate_callable_environment(
                        *environment,
                        application,
                        bindings,
                        state,
                    )?;
                    sources.insert(LatentSource::Captured {
                        unit: *unit,
                        environment,
                    });
                    continue;
                }
                crate::CallableReturnSource::Metadata(source) => {
                    if self.metadata_callable(*source).is_none() {
                        return Err(EffectSpecializationError::InvalidCallableReturnSources);
                    }
                    let mut source = *source;
                    if let Some(environment) = source.environment {
                        source.environment = Some(self.instantiate_callable_environment(
                            environment,
                            application,
                            bindings,
                            state,
                        )?);
                    } else if self.frozen_graphs.contains_key(&source.graph) {
                        source.environment = self.bind_frozen_graph_environment(
                            source.graph,
                            application,
                            bindings,
                            state,
                            invocation_environment,
                        )?;
                    }
                    sources.insert(LatentSource::Metadata(source));
                    continue;
                }
                crate::CallableReturnSource::AnonymousFlow(unit) => {
                    let EffectUnit::AnonymousFlow { owner, value, .. } = *unit else {
                        return Err(EffectSpecializationError::InvalidCallableReturnSources);
                    };
                    if self.anonymous_flow_unit_for_expr(owner, value) != Some(*unit) {
                        return Err(EffectSpecializationError::InvalidCallableReturnSources);
                    }
                    sources.insert(self.capture_native_callable(
                        *unit,
                        application,
                        bindings,
                        state,
                        invocation_environment,
                    )?);
                    continue;
                }
                crate::CallableReturnSource::SourceItem(item) => {
                    if let Some(etas_hir::HirItem::TopLevelLet(binding)) = self.hir.items.get(*item)
                    {
                        sources.extend(
                            self.latent_sources_for_symbol(state, binding.symbol)
                                .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?,
                        );
                        continue;
                    }
                    if !matches!(
                        self.hir.items.get(*item),
                        Some(
                            etas_hir::HirItem::Flow(_)
                                | etas_hir::HirItem::Tool(_)
                                | etas_hir::HirItem::Agent(_)
                        )
                    ) {
                        return Err(EffectSpecializationError::InvalidCallableReturnSources);
                    }
                    sources.insert(LatentSource::Unit(EffectUnit::Item(*item)));
                    continue;
                }
                crate::CallableReturnSource::Reference(reference) => {
                    let mut key = reference.clone();
                    key.environment = None;
                    let source = self
                        .callable_references
                        .get(&key)
                        .copied()
                        .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?;
                    sources.insert(self.capture_callable_reference(
                        source,
                        reference,
                        application,
                        bindings,
                        state,
                        invocation_environment,
                    )?);
                    continue;
                }
                crate::CallableReturnSource::Parameter(parameter) => parameter,
            };
            let argument = match bindings.get(&crate::ParameterCallTarget::Native(*parameter)) {
                Some(argument) => argument,
                None => {
                    let Some(SymbolDef::Param { owner, .. }) =
                        self.hir.symbols.get(*parameter).map(|data| &data.def)
                    else {
                        return Err(EffectSpecializationError::InvalidCallableReturnSources);
                    };
                    sources.insert(LatentSource::Parameter {
                        owner: *owner,
                        symbol: *parameter,
                    });
                    continue;
                }
            };
            sources.extend(self.trace_argument_origins_at(argument, state, application)?);
        }
        Ok(sources.into_iter().collect())
    }

    pub(crate) fn record_returned_callable_sources(
        &self,
        callee: EffectUnit,
        site: &CallSite<EffectUnit>,
        summary: &EffectSummary,
        state: &mut EffectState,
        environment: Option<crate::facts::CallableEnvironmentId>,
    ) -> Result<bool, EffectSpecializationError> {
        if !self.expr_type_contains_flow(site.call) && !self.expr_type_has_parameters(site.call) {
            return Ok(false);
        }
        let Some(exits) = &summary.exits else {
            return Ok(false);
        };
        let parameters = exits.callable_return_sources();
        if parameters.is_none() && exits.value_projections.is_empty() {
            return Ok(false);
        }
        let params = self
            .invocation_params(callee, environment)?
            .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?;
        let targets = params
            .iter()
            .map(|param| crate::ParameterCallTarget::Native(*param))
            .collect::<Vec<_>>();
        let mut bindings = environment
            .map(|id| self.native_environment_bindings(id))
            .transpose()?
            .unwrap_or_default();
        bindings.extend(self.call_trace_bindings(site, &targets)?);
        let mut paths = exits
            .value_projections
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        if let Some(ty) = self.types.facts.expr_types.get(&site.call)
            && let Some(actual_paths) = self.value_payload_paths(*ty)
        {
            paths.extend(actual_paths);
        }
        for path in paths {
            let selected = exits
                .value_projections
                .iter()
                .filter(|(prefix, _)| path.starts_with(prefix))
                .max_by_key(|(prefix, _)| prefix.len());
            let found = match selected {
                Some((prefix, crate::ProjectedReturnSources::Known(returned))) => self
                    .projected_return_bindings(
                        returned,
                        &path[prefix.len()..],
                        &bindings,
                        state,
                        site.call,
                        environment,
                    ),
                Some((_, crate::ProjectedReturnSources::Unavailable)) => None,
                None => parameters.as_ref().and_then(|returned| {
                    self.projected_return_bindings(
                        returned,
                        &path,
                        &bindings,
                        state,
                        site.call,
                        environment,
                    )
                }),
            };
            let sources = found
                .unwrap_or_else(|| [LatentSource::Unavailable { expr: site.call }].into())
                .into_iter()
                .collect::<Vec<_>>();
            let key = (site.call, path.clone());
            state
                .local_latent_values
                .value_projections
                .entry(key.clone())
                .or_default()
                .extend(sources.iter().copied());
            if !sources
                .iter()
                .any(|source| matches!(source, LatentSource::Unavailable { .. }))
            {
                self.discovered_call_results
                    .borrow_mut()
                    .value_projections
                    .entry(key)
                    .or_default()
                    .extend(sources);
            }
        }
        let projected_nodes = self
            .discovered_call_results
            .borrow()
            .value_projections
            .iter()
            .map(|((_, path), sources)| 1 + path.len() + sources.len())
            .sum::<usize>();
        if projected_nodes > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
            return Err(EffectSpecializationError::InvalidCallableReturnSources);
        }
        if let Some(parameters) = parameters {
            let sources = self.returned_callable_bindings(
                &parameters,
                &bindings,
                state,
                site.call,
                environment,
            )?;
            self.record_callable_result_sources(site.call, sources, state)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub(super) fn record_callable_result_sources(
        &self,
        call: HirExprId,
        sources: Vec<LatentSource>,
        state: &mut EffectState,
    ) -> Result<(), EffectSpecializationError> {
        if !sources.is_empty()
            && !sources
                .iter()
                .any(|source| matches!(source, LatentSource::Unavailable { .. }))
        {
            let mut results = self.discovered_call_results.borrow_mut();
            results.record_value_sources(call, sources.iter().copied());
            if results.values.len() + results.values.values().map(BTreeSet::len).sum::<usize>()
                > etas_package_metadata::MAX_METADATA_GRAPH_NODES
            {
                return Err(EffectSpecializationError::InvalidCallableEnvironment {
                    reason: "callable return-source graph exceeds the analysis node budget".into(),
                });
            }
        }
        state
            .local_latent_values
            .record_value_sources(call, sources);
        Ok(())
    }
}
