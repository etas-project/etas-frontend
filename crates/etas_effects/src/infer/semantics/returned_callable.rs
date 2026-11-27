use crate::infer::analysis::dependencies::InputFact;
use std::collections::BTreeSet;

use super::specialize::{EffectSpecializationError, TraceArgumentBindings};
use super::{engine::EffectSemantics, shared::*};

impl EffectSemantics<'_> {
    pub(super) fn pending_source_return(&self, unit: EffectUnit) -> bool {
        self.trace_equations.contains(unit)
            && matches!(
                unit.analysis_body_with_context(&self.context),
                etas_hir_analysis::interprocedural::HirAnalysisBody::Block(_)
                    | etas_hir_analysis::interprocedural::HirAnalysisBody::Expr(_)
                    | etas_hir_analysis::interprocedural::HirAnalysisBody::HandlerArm(_)
            )
    }

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
            if self.pending_source_return(callee) {
                // This source producer is an equation in the active component.
                // Before its first transfer its returned payload is bottom,
                // not an opaque unknown. The solver revisits all dependent
                // callers when concrete return alternatives become available.
                let ty = self.types.facts.expr_types[&site.call];
                let paths = self
                    .value_payload_paths(ty)
                    .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?;
                for path in paths {
                    state
                        .local_latent_values
                        .value_projections
                        .entry((site.call, path))
                        .or_default();
                }
                self.record_callable_result_sources(site.call, Vec::new(), state)?;
                return Ok(true);
            }
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
        bindings.extend(self.call_trace_bindings(site, &targets, state)?);
        let mut paths = exits
            .value_projections
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let actual_paths = self
            .types
            .facts
            .expr_types
            .get(&site.call)
            .and_then(|ty| self.value_payload_paths(*ty));
        paths.extend(actual_paths.iter().flatten().cloned());
        for path in paths {
            let selected = super::return_projection::returned_positions(
                exits
                    .value_projections
                    .iter()
                    .map(|(path, sources)| (path.as_slice(), sources)),
                &path,
            );
            let found = if selected.is_empty() {
                parameters.as_ref().and_then(|returned| {
                    self.projected_return_bindings(
                        returned,
                        &path,
                        &bindings,
                        state,
                        site.call,
                        environment,
                    )
                })
            } else {
                selected
                    .into_iter()
                    .try_fold(BTreeSet::new(), |mut all, (sources, suffix)| {
                        if let crate::ProjectedReturnSources::InjectedField(name) = sources {
                            let (field, suffix) = path.split_first()?;
                            let ty = self
                                .project_value(
                                    *self.types.facts.expr_types.get(&site.call)?,
                                    std::slice::from_ref(field),
                                )
                                .ok()??;
                            all.extend(
                                self.named_provider_sources(site.call, name, ty, suffix, state)?,
                            );
                            return Some(all);
                        }
                        let crate::ProjectedReturnSources::Known(sources) = sources else {
                            return None;
                        };
                        all.extend(self.projected_return_bindings(
                            sources,
                            &suffix,
                            &bindings,
                            state,
                            site.call,
                            environment,
                        )?);
                        Some(all)
                    })
            };
            let sources = found
                .unwrap_or_else(|| [LatentSource::Unavailable { expr: site.call }].into())
                .into_iter()
                .collect::<Vec<_>>();
            let key = (site.call, path.clone());
            let position = state
                .local_latent_values
                .value_projections
                .entry(key.clone())
                .or_default();
            if !sources
                .iter()
                .any(|source| matches!(source, LatentSource::Unavailable { .. }))
            {
                position.retain(|source| !matches!(source, LatentSource::Unavailable { expr } if *expr == site.call));
            }
            position.extend(sources.iter().copied());
            if !sources
                .iter()
                .any(|source| matches!(source, LatentSource::Unavailable { .. }))
            {
                self.discovered_call_results
                    .borrow_mut()
                    .extend_projection(key, sources);
            }
        }
        if self.discovered_call_results.borrow().projection_nodes
            > etas_package_metadata::MAX_METADATA_GRAPH_NODES
        {
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
        } else if let Some(paths) = actual_paths.filter(|paths| !paths.is_empty()) {
            // A record may have complete checked leaf origins even when its
            // aggregate return observation was not available in the template.
            let complete = paths.iter().try_fold(BTreeSet::new(), |mut all, path| {
                let sources = state
                    .local_latent_values
                    .value_projections
                    .get(&(site.call, path.clone()))?;
                if sources
                    .iter()
                    .any(|source| matches!(source, LatentSource::Unavailable { .. }))
                {
                    return None;
                }
                all.extend(sources.iter().copied());
                Some(all)
            });
            if let Some(sources) = complete {
                self.record_callable_result_sources(
                    site.call,
                    sources.into_iter().collect(),
                    state,
                )?;
                Ok(true)
            } else {
                Ok(false)
            }
        } else {
            Ok(false)
        }
    }

    pub(super) fn checked_value_sources(&self, expr: HirExprId) -> Option<&BTreeSet<LatentSource>> {
        self.dependencies.read(InputFact::Value(expr));
        self.call_results.value_sources(expr).filter(|sources| {
            !sources
                .iter()
                .any(|source| matches!(source, LatentSource::Unavailable { .. }))
        })
    }

    /// Read origins are collected from every control-flow state in a fresh
    /// solver round. They resolve deferred captures only when all states have
    /// checked origins; a known alternative never erases an unknown branch.
    pub(super) fn record_checked_read_sources(
        &mut self,
        expr: HirExprId,
        state: &EffectState,
    ) -> Result<(), EffectSpecializationError> {
        // A record used as a captured receiver can carry a pending origin too.
        // Track its complete sources across all control-flow states before
        // refining an earlier capture, as for ordinary reads.
        if !matches!(
            self.hir.exprs.get(expr),
            Some(
                etas_hir::HirExpr::Path(_)
                    | etas_hir::HirExpr::Record(_)
                    | etas_hir::HirExpr::Field { .. }
                    | etas_hir::HirExpr::Index { .. }
            )
        ) || (!self.expr_type_contains_flow(expr) && !self.expr_type_has_parameters(expr))
        {
            return Ok(());
        }
        let sources = self
            .latent_sources_for_expr(state, expr)
            .unwrap_or_else(|| vec![LatentSource::Unavailable { expr }]);
        if let Some(unit) = state.unit {
            self.checked_read_expressions
                .entry(unit)
                .or_default()
                .insert(expr);
        }
        let mut reads = self.discovered_call_results.borrow_mut();
        reads.update_value(expr, |current| {
            let previous = current.clone();
            current.extend(sources);
            if !previous.is_empty() && previous.len() != current.len() {
                self.close_origin_set(&previous, current);
            }
        });
        if reads.value_nodes > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
            return Err(EffectSpecializationError::InvalidCallableEnvironment {
                reason: "checked read-source graph exceeds the analysis node budget".into(),
            });
        }
        Ok(())
    }

    pub(super) fn record_callable_result_sources(
        &self,
        call: HirExprId,
        sources: Vec<LatentSource>,
        state: &mut EffectState,
    ) -> Result<(), EffectSpecializationError> {
        let complete = !sources
            .iter()
            .any(|source| matches!(source, LatentSource::Unavailable { .. }));
        if complete {
            let mut results = self.discovered_call_results.borrow_mut();
            results.update_value(call, |values| values.extend(sources.iter().copied()));
            if results.value_nodes > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
                return Err(EffectSpecializationError::InvalidCallableEnvironment {
                    reason: "callable return-source graph exceeds the analysis node budget".into(),
                });
            }
        }
        if complete && let Some(previous) = state.local_latent_values.values.get_mut(&call) {
            previous.retain(
                |source| !matches!(source, LatentSource::Unavailable { expr } if *expr == call),
            );
        }
        state
            .local_latent_values
            .record_value_sources(call, sources);
        Ok(())
    }
}
