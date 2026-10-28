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
            sources.extend(self.trace_argument_origins(argument, state)?);
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
        let Some(exits) = &summary.exits else {
            return Ok(false);
        };
        let Some(parameters) = exits.callable_return_sources() else {
            return Ok(false);
        };
        let params = self
            .unit_params(callee)
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
        let sources =
            self.returned_callable_bindings(&parameters, &bindings, state, site.call, environment)?;
        self.record_callable_result_sources(site.call, sources, state)?;
        Ok(true)
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
