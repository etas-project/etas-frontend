use super::*;
use etas_effects::{ParameterCallTarget, facts::MetadataCallableAnalysis};

impl Builder<'_, '_> {
    pub(super) fn reserve_foreign(
        &mut self,
        instance: ForeignInstance,
    ) -> Result<u32, PackageMetadataError> {
        if let Some(index) = self.foreign.get(&instance) {
            return Ok(*index);
        }
        if self.values.len() >= etas_package_metadata::MAX_METADATA_GRAPH_NODES {
            return Err(invalid(
                "composed callable graph exceeds the metadata node budget",
            ));
        }
        let index = u32::try_from(self.values.len())
            .map_err(|_| invalid("composed callable graph exceeds u32"))?;
        self.values.push(None);
        self.origins
            .push(crate::package_metadata::CallableGraphOrigin::Metadata(
                instance.source,
            ));
        self.foreign.insert(instance, index);
        self.parents.insert(index, instance.root);
        self.pending.push(Pending::Metadata(index, instance));
        Ok(index)
    }

    fn foreign_scope(
        &mut self,
        instance: ForeignInstance,
        scope: CallableValueScope,
    ) -> Result<CallableValueScope, PackageMetadataError> {
        Ok(match scope {
            CallableValueScope::Root => instance.root,
            CallableValueScope::Value(value) => {
                CallableValueScope::Value(self.reserve_foreign(ForeignInstance {
                    source: etas_effects::MetadataCallableSource {
                        value,
                        ..instance.source
                    },
                    ..instance
                })?)
            }
        })
    }

    pub(super) fn foreign_node(
        &mut self,
        index: u32,
        instance: ForeignInstance,
    ) -> Result<Option<CallableValueNode>, PackageMetadataError> {
        let checked = self.projection.checked;
        let graph = checked
            .effects
            .metadata_callable_graphs
            .get(&instance.source.graph)
            .ok_or_else(|| invalid("exported metadata callable has no checked graph fact"))?;
        let node = graph
            .graph
            .values
            .get(instance.source.value as usize)
            .ok_or_else(|| invalid("exported metadata callable has no graph node"))?;
        let signature = graph
            .signatures
            .get(instance.source.value as usize)
            .ok_or_else(|| invalid("exported metadata callable has no checked signature"))?;
        let analysis = checked
            .effects
            .metadata_callables
            .get(&instance.source)
            .ok_or_else(|| invalid("exported metadata callable has no materialized analysis"))?;
        let MetadataCallableAnalysis::Solved {
            summary,
            type_bindings,
            effect_row_bindings,
        } = analysis
        else {
            return Ok(None);
        };
        let parent_scope = self.foreign_scope(instance, node.parent_scope)?;
        self.parents.insert(index, parent_scope);
        let environment = instance
            .source
            .environment
            .map(|id| {
                checked
                    .effects
                    .callable_environments
                    .get(&id)
                    .ok_or_else(|| invalid("exported metadata callable has no capture environment"))
            })
            .transpose()?;
        let mut generics =
            self.generic_context(environment.map(|environment| environment.owner))?;
        for binding in &node.generic_bindings {
            if let Some((_, ty)) = type_bindings
                .iter()
                .find(|(label, _)| label == &binding.label)
            {
                generics.owner_type_names.insert(binding.label.clone());
                let mut ty = self.projection.metadata_type(*ty)?;
                normalize_generic_type(&mut ty, &generics.type_names);
                generics.types.insert(binding.label.clone(), ty);
            } else if let Some((_, row)) = effect_row_bindings
                .iter()
                .find(|(label, _)| label == &binding.label)
            {
                generics.rows.insert(
                    binding.label.clone(),
                    self.projection.effect_row_metadata(
                        row,
                        &generics.type_names,
                        &generics.effect_names,
                    )?,
                );
            } else {
                let CallableValueScope::Value(value) = binding.scope else {
                    return Err(invalid(
                        "exported metadata callable has an unbound root generic",
                    ));
                };
                let parameter = graph
                    .graph
                    .values
                    .get(value as usize)
                    .and_then(|node| node.generic_params.get(binding.position as usize))
                    .ok_or_else(|| invalid("exported metadata generic has no checked binder"))?;
                if generics
                    .bindings
                    .iter()
                    .any(|current| current.label == binding.label)
                {
                    return Err(invalid(
                        "unbound metadata generic label conflicts with the caller binder namespace",
                    ));
                }
                generics
                    .bindings
                    .push(etas_package_metadata::CallableValueGenericBinding {
                        label: binding.label.clone(),
                        scope: self.foreign_scope(instance, binding.scope)?,
                        position: binding.position,
                    });
                match parameter.kind {
                    etas_package_metadata::GenericParamKind::Type => {
                        generics.type_names.insert(binding.label.clone());
                        generics.owner_type_names.insert(binding.label.clone());
                    }
                    etas_package_metadata::GenericParamKind::Effect => {
                        generics.effect_names.insert(binding.label.clone());
                    }
                }
            }
        }
        let scope = CallableValueScope::Value(index);
        let returned_sources = if let Some(returned) = &node.returned_sources {
            let mut output = BTreeSet::new();
            for source in returned {
                match *source {
                    CallableValueSource::Value(value) => {
                        output.insert(CallableValueSource::Value(self.reserve_foreign(
                            ForeignInstance {
                                source: etas_effects::MetadataCallableSource {
                                    value,
                                    ..instance.source
                                },
                                ..instance
                            },
                        )?));
                    }
                    CallableValueSource::Parameter(parameter) => {
                        if parameter.scope == CallableValueScope::Value(instance.source.value) {
                            output.insert(CallableValueSource::Parameter(CallableValueParameter {
                                scope,
                                position: parameter.position,
                            }));
                            continue;
                        }
                        let target = ParameterCallTarget::Graph {
                            graph: instance.source.graph,
                            parameter,
                        };
                        if let Some(origins) =
                            environment.and_then(|environment| environment.parameters.get(&target))
                        {
                            if origins.is_empty() {
                                return Ok(None);
                            }
                            for origin in origins {
                                let Some((source, environment)) = self.checked_origin(*origin)?
                                else {
                                    return Ok(None);
                                };
                                let Some(sources) =
                                    self.sources(&[source].into(), scope, environment)?
                                else {
                                    return Ok(None);
                                };
                                output.extend(sources);
                            }
                        } else {
                            if parameter.scope == CallableValueScope::Root {
                                return Err(invalid(
                                    "returned metadata parameter has no checked root argument",
                                ));
                            }
                            output.insert(CallableValueSource::Parameter(CallableValueParameter {
                                scope: self.foreign_scope(instance, parameter.scope)?,
                                position: parameter.position,
                            }));
                        }
                    }
                }
            }
            Some(output.into_iter().collect())
        } else {
            None
        };
        let mut labels = BTreeMap::new();
        let mut parameters = BTreeMap::new();
        for target in trace_parameters(&summary.action_trace) {
            let parameter = match target {
                ParameterCallTarget::Native(symbol) => match self.parameter(symbol, scope)? {
                    Some(parameter) => parameter,
                    None => return Ok(None),
                },
                ParameterCallTarget::Graph { graph, parameter }
                    if graph == instance.source.graph
                        && parameter.scope != CallableValueScope::Root =>
                {
                    CallableValueParameter {
                        scope: self.foreign_scope(instance, parameter.scope)?,
                        position: parameter.position,
                    }
                }
                _ => return Ok(None),
            };
            if !self.scope_contains(scope, parameter.scope) {
                return Ok(None);
            }
            let label = match parameter.scope {
                CallableValueScope::Root => format!("rp{}", parameter.position),
                CallableValueScope::Value(value) => format!("v{value}p{}", parameter.position),
            };
            labels.insert(target, label.clone());
            parameters.insert(parameter, label);
        }
        let Some(projected) = summary::project(
            self.projection,
            summary,
            &generics.type_names,
            &generics.effect_names,
            &labels,
        )?
        else {
            return Ok(None);
        };
        Ok(Some(CallableValueNode {
            handler_requests: projected.handler_requests,
            behavior: projected.behavior,
            parent_scope,
            generic_params: node.generic_params.clone(),
            generic_bindings: generics.bindings.clone(),
            param_names: node.param_names.clone(),
            params: signature
                .params
                .iter()
                .map(|ty| generics.metadata_type(self.projection, *ty))
                .collect::<Result<_, _>>()?,
            output: generics.metadata_type(self.projection, signature.output)?,
            public_effects: projected.public_effects,
            requested_actions: projected.requested_actions,
            handled_requested_actions: projected.handled_requested_actions,
            default_handler_effects: projected.default_handler_effects,
            parameter_bindings: parameters
                .into_iter()
                .map(
                    |(parameter, label)| etas_package_metadata::CallableValueParameterBinding {
                        label,
                        parameter,
                    },
                )
                .collect(),
            action_trace: projected.action_trace,
            returned_sources,
        }))
    }
}
