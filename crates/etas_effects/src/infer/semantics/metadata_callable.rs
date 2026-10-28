use super::{
    callable_summary::{CallableSummaryInstantiation, CallableSummaryMode, CallableSummaryPlan},
    engine::EffectSemantics,
    shared::*,
    specialize::EffectSpecializationError,
};
use etas_package_metadata::{CallableValueScope, CallableValueSource};
use etas_types::{Assignable, TypeRelation};
use std::collections::BTreeMap;

pub(super) struct MetadataCallBindings<'a> {
    pub key: Option<etas_code::CodeFingerprint>,
    pub scope: CallableValueScope,
    pub environment: Option<crate::facts::CallableEnvironmentId>,
    pub arguments: &'a [HirExprId],
    pub generics: &'a [(String, etas_types::CallableGenericParamKind)],
    pub type_bindings: &'a [(String, TypeId)],
    pub effect_row_bindings: &'a [(String, EffectRow)],
}

impl EffectSemantics<'_> {
    pub(crate) fn bind_frozen_graph_environment(
        &self,
        graph: etas_code::CodeFingerprint,
        application: HirExprId,
        bindings: &super::specialize::TraceArgumentBindings,
        state: &EffectState,
        inherited: Option<crate::facts::CallableEnvironmentId>,
    ) -> Result<Option<crate::facts::CallableEnvironmentId>, EffectSpecializationError> {
        let proof = self
            .frozen_graphs
            .get(&graph)
            .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?;
        let mut parameters = BTreeMap::new();
        for (position, symbol) in self
            .unit_params(EffectUnit::Item(proof.identity.owner))
            .ok_or(EffectSpecializationError::InvalidCallableParameterInterface)?
            .iter()
            .enumerate()
        {
            if let Some(binding) = bindings.get(&crate::ParameterCallTarget::Native(*symbol)) {
                parameters.insert(
                    crate::ParameterCallTarget::Graph {
                        graph,
                        parameter: etas_package_metadata::CallableValueParameter {
                            scope: CallableValueScope::Root,
                            position: position as u32,
                        },
                    },
                    self.trace_argument_origins(binding, state)?,
                );
            }
        }
        let generics = self
            .native_creation_generics(proof.identity.owner, application, state, inherited)?
            .into_iter()
            .map(|(parameter, value)| match parameter {
                crate::facts::CallableGenericParameter::Native { position, .. } => Ok((
                    crate::facts::CallableGenericParameter::Graph {
                        graph,
                        scope: CallableValueScope::Root,
                        position,
                    },
                    value,
                )),
                _ => Err(EffectSpecializationError::InvalidCallableReturnSources),
            })
            .collect::<Result<_, _>>()?;
        let environment = crate::facts::CallableEnvironment {
            owner: state
                .owner
                .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?,
            parameters,
            generics,
        };
        if environment.parameters.is_empty() && environment.generics.is_empty() {
            return Ok(None);
        }
        let id = crate::facts::CallableEnvironmentId {
            origin: graph,
            application: self.callable_environment_site(application)?,
        };
        self.record_callable_environment(id, environment)?;
        Ok(Some(id))
    }

    pub(super) fn callable_graph(
        &self,
        key: etas_code::CodeFingerprint,
    ) -> Option<(
        &etas_package_metadata::CallableValueGraph,
        &[crate::ExternalCallableValueMetadata],
        Span,
    )> {
        if let Some(graph) = self.frozen_graphs.get(&key) {
            if self.metadata_callables.contains_key(&key) {
                return None;
            }
            return (graph.graph.values.len() == graph.values.len()
                && graph.values.len() == graph.exits.len())
            .then_some((&graph.graph, &graph.values, graph.anchor));
        }
        let index = self.metadata_callables.get(&key).copied().flatten()?;
        let metadata = self.external_summaries.get(index)?;
        let graph = metadata.returned_callables.as_ref()?;
        (graph.values.len() == metadata.callable_values.len()).then_some((
            graph,
            &metadata.callable_values,
            metadata.span,
        ))
    }

    pub(crate) fn metadata_callable(
        &self,
        source: crate::MetadataCallableSource,
    ) -> Option<(
        &etas_package_metadata::CallableValueGraph,
        &etas_package_metadata::CallableValueNode,
        &crate::ExternalCallableValueMetadata,
    )> {
        let (graph, values, _) = self.callable_graph(source.graph)?;
        Some((
            graph,
            graph.values.get(source.value as usize)?,
            values.get(source.value as usize)?,
        ))
    }

    pub(super) fn prepare_metadata_value_summary(
        &self,
        source: crate::MetadataCallableSource,
        span: Span,
        mode: CallableSummaryMode,
    ) -> Result<CallableSummaryPlan, EffectSpecializationError> {
        let (_, node, checked) = self
            .metadata_callable(source)
            .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?;
        let parameter_targets = node
            .parameter_bindings
            .iter()
            .map(|binding| {
                (
                    binding.label.clone(),
                    crate::ParameterCallTarget::Graph {
                        graph: source.graph,
                        parameter: binding.parameter,
                    },
                )
            })
            .collect::<Vec<_>>();
        let mut summary = self
            .external_callable_summary(
                crate::pipeline::ExternalCallableEffectView {
                    handler_requests: checked.handler_requests.as_ref(),
                    parameter_targets: &parameter_targets,
                    public_effects: &checked.public_effects,
                    requested_actions: &checked.requested_actions,
                    handled_requested_actions: &checked.handled_requested_actions,
                    default_handler_effects: Some(&checked.default_handler_effects),
                    action_trace: &checked.action_trace,
                },
                span,
            )
            .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?;
        if let Some(proof) = self.frozen_graphs.get(&source.graph) {
            summary.exits = proof
                .exits
                .get(source.value as usize)
                .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?
                .clone();
        }
        let (determinism, support) = behavior::lower(&node.behavior);
        if summary.determinism > determinism || !summary.support.less_equal(&support) {
            return Err(EffectSpecializationError::InvalidCallableReturnSources);
        }
        summary.determinism = determinism;
        summary.support = support;
        let (types, rows) = self.metadata_environment_generics_in_mode(source, mode)?;
        summary = self.specialize_summary_with_bindings(&summary, &[], &types, &rows)?;
        let environment = source
            .environment
            .map(|id| self.callable_environment(id))
            .transpose()?;
        let scopes = self.metadata_callable_scopes(source)?;
        let own_parameters = node
            .parameter_bindings
            .iter()
            .filter(|binding| {
                binding.parameter.scope == CallableValueScope::Value(source.value)
                    || (mode == CallableSummaryMode::Descriptor
                        && binding.parameter.scope != CallableValueScope::Root
                        && scopes.contains(&binding.parameter.scope))
            })
            .map(|binding| crate::ParameterCallTarget::Graph {
                graph: source.graph,
                parameter: binding.parameter,
            })
            .collect();
        Ok(CallableSummaryPlan::Captured(
            CallableSummaryInstantiation {
                template: summary,
                bindings: environment.map_or_else(BTreeMap::new, |environment| {
                    environment
                        .parameters
                        .into_iter()
                        .filter(|(target, _)| is_captured_parameter(*target, source, &scopes))
                        .collect()
                }),
                own_parameters,
                type_bindings: types,
            },
        ))
    }

    pub(super) fn metadata_callable_scopes(
        &self,
        source: crate::MetadataCallableSource,
    ) -> Result<BTreeSet<CallableValueScope>, EffectSpecializationError> {
        let (graph, _, _) = self
            .metadata_callable(source)
            .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?;
        let mut scope = CallableValueScope::Value(source.value);
        let mut scopes = BTreeSet::new();
        loop {
            if !scopes.insert(scope)
                || scopes.len() > etas_package_metadata::MAX_METADATA_GRAPH_DEPTH + 1
            {
                return Err(EffectSpecializationError::InvalidCallableEnvironment {
                    reason: "metadata callable has an invalid lexical scope graph".into(),
                });
            }
            let CallableValueScope::Value(value) = scope else {
                return Ok(scopes);
            };
            scope = graph
                .values
                .get(value as usize)
                .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?
                .parent_scope;
        }
    }

    pub(super) fn record_metadata_sources(
        &self,
        context: MetadataCallBindings<'_>,
        sources: &[CallableValueSource],
        state: &mut EffectState,
        output: HirExprId,
    ) -> Result<(), EffectSpecializationError> {
        if sources.is_empty() {
            return Err(EffectSpecializationError::InvalidCallableReturnSources);
        }
        let (key, environment, environment_id) =
            self.bind_metadata_environment(context, state, output)?;
        let mut found = BTreeSet::new();
        for source in sources {
            match source {
                CallableValueSource::Value(value) => {
                    let source = crate::MetadataCallableSource {
                        graph: key,
                        value: *value,
                        environment: environment_id,
                    };
                    self.metadata_callable(source)
                        .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?;
                    found.insert(LatentSource::Metadata(source));
                }
                CallableValueSource::Parameter(parameter) => {
                    let origins = environment
                        .parameters
                        .get(&crate::ParameterCallTarget::Graph {
                            graph: key,
                            parameter: *parameter,
                        })
                        .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?;
                    found.extend(origins);
                }
            }
        }
        state
            .local_latent_values
            .record_value_sources(output, found);
        Ok(())
    }

    fn bind_metadata_environment(
        &self,
        context: MetadataCallBindings<'_>,
        state: &EffectState,
        output: HirExprId,
    ) -> Result<
        (
            etas_code::CodeFingerprint,
            crate::facts::CallableEnvironment,
            Option<crate::facts::CallableEnvironmentId>,
        ),
        EffectSpecializationError,
    > {
        let key = context
            .key
            .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?;
        let mut environment = match context.environment {
            Some(id) => self.callable_environment(id)?,
            None => crate::facts::CallableEnvironment {
                owner: state
                    .owner
                    .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?,
                parameters: Default::default(),
                generics: Default::default(),
            },
        };
        environment.owner = state
            .owner
            .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?;
        for (position, (name, kind)) in context.generics.iter().enumerate() {
            use crate::facts::{CallableGenericParameter as P, CallableGenericValue as V};
            let value = match kind {
                etas_types::CallableGenericParamKind::Type => V::Type(
                    context
                        .type_bindings
                        .iter()
                        .find(|(param, _)| param == name)
                        .map(|(_, ty)| *ty)
                        .ok_or_else(|| EffectSpecializationError::MissingTypeBinding {
                            param: name.clone(),
                        })?,
                ),
                etas_types::CallableGenericParamKind::Effect => V::EffectRow(
                    context
                        .effect_row_bindings
                        .iter()
                        .find(|(param, _)| param == name)
                        .map(|(_, row)| row.clone())
                        .ok_or_else(|| EffectSpecializationError::MissingEffectRowBinding {
                            param: name.clone(),
                        })?,
                ),
            };
            environment.generics.insert(
                P::Graph {
                    graph: key,
                    scope: context.scope,
                    position: position as u32,
                },
                value,
            );
        }
        for (position, expr) in context.arguments.iter().enumerate() {
            if self.expr_type_contains_flow(*expr) {
                let origins = self
                    .latent_sources_for_expr(state, *expr)
                    .unwrap_or_else(|| vec![LatentSource::Unavailable { expr: *expr }]);
                environment.parameters.insert(
                    crate::ParameterCallTarget::Graph {
                        graph: key,
                        parameter: etas_package_metadata::CallableValueParameter {
                            scope: context.scope,
                            position: position as u32,
                        },
                    },
                    origins.into_iter().collect(),
                );
            }
        }
        let environment_id = if environment.parameters.is_empty() && environment.generics.is_empty()
        {
            None
        } else {
            let application = self.callable_environment_site(output)?;
            let origin = context.environment.map(|id| id.origin).unwrap_or_else(|| {
                let mut bytes = Vec::with_capacity(64);
                bytes.extend_from_slice(key.as_bytes());
                bytes.extend_from_slice(application.as_bytes());
                etas_code::CodeFingerprint::of_bytes(
                    "etas.effects.metadata-closure-origin.v1",
                    &bytes,
                )
            });
            let id = crate::facts::CallableEnvironmentId {
                origin,
                application,
            };
            self.record_callable_environment(id, environment.clone())?;
            Some(id)
        };
        Ok((key, environment, environment_id))
    }

    pub(crate) fn metadata_value_call(
        &self,
        mut source: crate::MetadataCallableSource,
        site: &CallSite<EffectUnit>,
        state: &mut EffectState,
    ) -> Result<EffectSummary, EffectSpecializationError> {
        let (_, node, checked) = self
            .metadata_callable(source)
            .ok_or(EffectSpecializationError::InvalidCallableReturnSources)?;
        let generics = node
            .generic_params
            .iter()
            .map(|parameter| {
                (
                    parameter.name.clone(),
                    match parameter.kind {
                        etas_package_metadata::GenericParamKind::Type => {
                            etas_types::CallableGenericParamKind::Type
                        }
                        etas_package_metadata::GenericParamKind::Effect => {
                            etas_types::CallableGenericParamKind::Effect
                        }
                    },
                )
            })
            .collect::<Vec<_>>();
        let (call_types, call_rows) = self.checked_call_generic_values(site.call, state)?;
        let arguments = self.call_arguments(site, node.params.len())?;
        if !generics.is_empty() {
            source.environment = self
                .bind_metadata_environment(
                    MetadataCallBindings {
                        key: Some(source.graph),
                        scope: CallableValueScope::Value(source.value),
                        environment: source.environment,
                        arguments: &arguments,
                        generics: &generics,
                        type_bindings: &call_types,
                        effect_row_bindings: &call_rows,
                    },
                    state,
                    site.call,
                )?
                .2;
        }
        let signature = self
            .flow_type_for_expr(site.callee_expr)
            .ok_or(EffectSpecializationError::InvalidCallableParameterInterface)?;
        let (types, _) = self.metadata_environment_generics(source)?;
        let substitutions = types.iter().cloned().collect();
        let params = checked
            .params
            .iter()
            .map(|ty| {
                etas_types::substitute_named_params_in_store(&self.types.store, *ty, &substitutions)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let output = etas_types::substitute_named_params_in_store(
            &self.types.store,
            checked.output,
            &substitutions,
        )?;
        let relation = TypeRelation::new(&self.types.store);
        let equivalent = |left, right| {
            relation.assignable(left, right).is_ok() && relation.assignable(right, left).is_ok()
        };
        if signature.input.len() != checked.params.len()
            || !signature
                .input
                .iter()
                .zip(&params)
                .all(|(left, right)| equivalent(*left, *right))
            || !equivalent(signature.output, output)
        {
            return Err(EffectSpecializationError::InvalidCallableParameterInterface);
        }
        let mut summary = self.latent_source_summary(LatentSource::Metadata(source), site.span)?;
        let targets = (0..node.params.len())
            .map(|position| crate::ParameterCallTarget::Graph {
                graph: source.graph,
                parameter: etas_package_metadata::CallableValueParameter {
                    scope: CallableValueScope::Value(source.value),
                    position: position as u32,
                },
            })
            .collect::<Vec<_>>();
        let bindings = self.call_trace_bindings(site, &targets)?;
        let inferred = self.inferred_callback_parameters(&targets, &params)?;
        let template = summary.action_trace.clone();
        self.merge_parameter_call_summaries(&mut summary, &template, &bindings, state, &inferred)?;
        summary.action_trace = self.specialize_parameter_call_trace(&template, &bindings, state)?;
        if let Some(bound) = &signature.effects {
            let coverage = crate::EffectCoverage {
                registry: self.registry,
                types: &self.types.store,
            };
            if !coverage.row_covers(&self.row_from_type_ref(bound), &summary.escaping_effects) {
                return Err(EffectSpecializationError::InvalidCallableReturnSources);
            }
        }
        if let Some(returned) = &node.returned_sources {
            self.record_metadata_sources(
                MetadataCallBindings {
                    key: Some(source.graph),
                    scope: CallableValueScope::Value(source.value),
                    environment: source.environment,
                    arguments: &arguments,
                    generics: &generics,
                    type_bindings: &call_types,
                    effect_row_bindings: &call_rows,
                },
                returned,
                state,
                site.call,
            )?;
        } else if self.flow_type_for_expr(site.call).is_some() {
            state
                .local_latent_values
                .record_value_sources(site.call, [LatentSource::Unavailable { expr: site.call }]);
        }
        Ok(summary)
    }
}

mod behavior;

fn is_captured_parameter(
    target: crate::ParameterCallTarget,
    source: crate::MetadataCallableSource,
    scopes: &BTreeSet<CallableValueScope>,
) -> bool {
    matches!(target, crate::ParameterCallTarget::Graph { graph, parameter }
        if graph == source.graph && parameter.scope != CallableValueScope::Value(source.value) && scopes.contains(&parameter.scope))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocation_parameters_shadow_old_environment_entries() {
        let graph = etas_code::CodeFingerprint::of_bytes("test.metadata-captures", b"graph");
        let other = etas_code::CodeFingerprint::of_bytes("test.metadata-captures", b"other");
        let source = crate::MetadataCallableSource {
            graph,
            value: 1,
            environment: None,
        };
        let scopes = [
            CallableValueScope::Root,
            CallableValueScope::Value(0),
            CallableValueScope::Value(1),
        ]
        .into();
        let target = |graph, scope| crate::ParameterCallTarget::Graph {
            graph,
            parameter: etas_package_metadata::CallableValueParameter { scope, position: 0 },
        };
        assert!(is_captured_parameter(
            target(graph, CallableValueScope::Root),
            source,
            &scopes
        ));
        assert!(is_captured_parameter(
            target(graph, CallableValueScope::Value(0)),
            source,
            &scopes
        ));
        assert!(!is_captured_parameter(
            target(graph, CallableValueScope::Value(1)),
            source,
            &scopes
        ));
        assert!(!is_captured_parameter(
            target(graph, CallableValueScope::Value(2)),
            source,
            &scopes
        ));
        assert!(!is_captured_parameter(
            target(other, CallableValueScope::Root),
            source,
            &scopes
        ));
    }
}
