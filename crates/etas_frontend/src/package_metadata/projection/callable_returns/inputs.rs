//! Preserve application edges while rebasing a callable graph's lexical scopes.
use super::*;
use etas_package_metadata::{CallableCallArgument, CallableCallInputs, CallableReturnedProjection};

impl Builder<'_, '_> {
    pub(super) fn reserve_inputs(
        &self,
        instance: InputInstance,
    ) -> Result<u32, PackageMetadataError> {
        let mut instances = self.input_instances.borrow_mut();
        if let Some(index) = instances.get(&instance) {
            return Ok(*index);
        }
        if instances.len() >= etas_package_metadata::MAX_METADATA_GRAPH_NODES {
            return Err(invalid("callback inputs exceed the metadata node budget"));
        }
        let index = instances.len() as u32;
        instances.insert(instance, index);
        self.pending_inputs.borrow_mut().push((index, instance));
        Ok(index)
    }

    fn input_origins(
        &mut self,
        origins: &BTreeSet<etas_effects::facts::CallableValueOrigin>,
        scope: CallableValueScope,
    ) -> Result<Option<Vec<CallableValueSource>>, PackageMetadataError> {
        let mut output = BTreeSet::new();
        for origin in origins {
            let Some((source, environment)) = self.checked_origin(*origin)? else {
                return Ok(None);
            };
            let Some(sources) = self.sources(&[source].into(), scope, environment)? else {
                return Ok(None);
            };
            output.extend(sources);
        }
        Ok(Some(output.into_iter().collect()))
    }

    pub(super) fn input_node(
        &mut self,
        instance: InputInstance,
    ) -> Result<CallableCallInputs, PackageMetadataError> {
        match instance {
            InputInstance::Native { id, scope } => {
                let inputs = self
                    .projection
                    .checked
                    .effects
                    .parameter_projections
                    .values()
                    .find_map(|projection| projection.call_inputs.get(&id))
                    .cloned()
                    .ok_or_else(|| {
                        invalid("exported callback application has no checked inputs")
                    })?;
                let mut arguments = Vec::new();
                for argument in &inputs.arguments {
                    let sources = self.input_origins(&argument.sources, scope)?;
                    let mut projections = Vec::new();
                    for projection in &argument.projections {
                        projections.push(CallableReturnedProjection {
                            path: projection.path.clone(),
                            sources: self.input_origins(&projection.sources, scope)?,
                        });
                    }
                    projections.sort_by(|a, b| a.path.cmp(&b.path));
                    arguments.push(CallableCallArgument {
                        name: argument.name.clone(),
                        sources,
                        projections,
                    });
                }
                Ok(CallableCallInputs { scope, arguments })
            }
            InputInstance::Foreign { id, instance } => {
                let graph = self
                    .projection
                    .checked
                    .effects
                    .metadata_callable_graphs
                    .get(&instance.source.graph)
                    .ok_or_else(|| invalid("exported callback inputs have no checked graph"))?;
                let inputs = graph
                    .graph
                    .call_inputs
                    .get(id as usize)
                    .cloned()
                    .ok_or_else(|| invalid("foreign callback input reference is out of range"))?;
                let scope = self.foreign_scope(instance, inputs.scope)?;
                let mut arguments = Vec::new();
                for argument in inputs.arguments {
                    let sources = argument
                        .sources
                        .as_ref()
                        .map(|sources| self.foreign_input_sources(instance, scope, sources))
                        .transpose()?;
                    let mut projections = Vec::new();
                    for projection in argument.projections {
                        projections.push(CallableReturnedProjection {
                            path: projection.path,
                            sources: projection
                                .sources
                                .as_ref()
                                .map(|sources| self.foreign_input_sources(instance, scope, sources))
                                .transpose()?,
                        });
                    }
                    arguments.push(CallableCallArgument {
                        name: argument.name,
                        sources,
                        projections,
                    });
                }
                Ok(CallableCallInputs { scope, arguments })
            }
        }
    }

    fn foreign_input_sources(
        &mut self,
        instance: ForeignInstance,
        lexical_scope: CallableValueScope,
        sources: &[CallableValueSource],
    ) -> Result<Vec<CallableValueSource>, PackageMetadataError> {
        let mut output = BTreeSet::new();
        for source in sources {
            output.insert(match *source {
                CallableValueSource::Value(value) | CallableValueSource::FrozenContract(value) => {
                    let index = self.reserve_foreign(ForeignInstance {
                        source: etas_effects::MetadataCallableSource {
                            value,
                            ..instance.source
                        },
                        ..instance
                    })?;
                    if matches!(source, CallableValueSource::FrozenContract(_)) {
                        CallableValueSource::FrozenContract(index)
                    } else {
                        CallableValueSource::Value(index)
                    }
                }
                CallableValueSource::Existential(object) => CallableValueSource::Existential(
                    self.reserve_foreign_object(ForeignObjectInstance {
                        source: etas_effects::MetadataObjectSource {
                            graph: instance.source.graph,
                            object,
                            environment: instance.source.environment,
                        },
                        root: instance.root,
                    })?,
                ),
                CallableValueSource::Parameter(parameter) => {
                    let target = etas_effects::ParameterCallTarget::Graph {
                        graph: instance.source.graph,
                        parameter,
                    };
                    if let Some(origins) = instance
                        .source
                        .environment
                        .and_then(|id| {
                            self.projection
                                .checked
                                .effects
                                .callable_environments
                                .get(&id)
                        })
                        .and_then(|environment| environment.parameters.get(&target))
                        .cloned()
                    {
                        output.extend(self.input_origins(&origins, lexical_scope)?.ok_or_else(
                            || invalid("foreign callback input has unavailable captured origins"),
                        )?);
                        continue;
                    }
                    let scope = self.foreign_scope(instance, parameter.scope)?;
                    CallableValueSource::Parameter(
                        self.foreign_parameter(instance, parameter, scope)?,
                    )
                }
            });
        }
        Ok(output.into_iter().collect())
    }
}
