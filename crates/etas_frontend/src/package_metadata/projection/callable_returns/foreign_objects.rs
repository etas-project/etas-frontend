use super::*;
use etas_effects::{
    ParameterCallTarget,
    facts::{CallableGenericParameter, CallableGenericValue},
};
use etas_package_metadata::{
    ExistentialMemberWitness, ExistentialValueNode, ExistentialWitnessTarget,
};

impl Builder<'_, '_> {
    pub(super) fn reserve_foreign_object(
        &mut self,
        instance: ForeignObjectInstance,
    ) -> Result<u32, PackageMetadataError> {
        if let Some(index) = self.foreign_objects.get(&instance) {
            return Ok(*index);
        }
        if self.objects.len() + self.values.len() >= etas_package_metadata::MAX_METADATA_GRAPH_NODES
        {
            return Err(invalid(
                "composed value evidence exceeds the metadata node budget",
            ));
        }
        let index = u32::try_from(self.objects.len())
            .map_err(|_| invalid("object evidence index exceeds u32"))?;
        self.objects.push(None);
        self.foreign_objects.insert(instance, index);
        self.pending.push(Pending::ForeignObject(index, instance));
        Ok(index)
    }

    fn object_callable(
        &mut self,
        instance: ForeignObjectInstance,
        value: u32,
        frozen: bool,
    ) -> Result<u32, PackageMetadataError> {
        self.reserve_foreign(ForeignInstance {
            source: etas_effects::MetadataCallableSource {
                graph: instance.source.graph,
                value,
                environment: instance.source.environment,
                analysis: if frozen {
                    etas_effects::CallableAnalysisKind::FrozenContract
                } else {
                    etas_effects::CallableAnalysisKind::CheckedRevision
                },
            },
            root: instance.root,
        })
    }

    fn object_scope(
        &mut self,
        instance: ForeignObjectInstance,
        scope: CallableValueScope,
    ) -> Result<CallableValueScope, PackageMetadataError> {
        Ok(match scope {
            CallableValueScope::Root => instance.root,
            CallableValueScope::Value(value) => {
                CallableValueScope::Value(self.object_callable(instance, value, false)?)
            }
        })
    }

    pub(super) fn foreign_object_node(
        &mut self,
        instance: ForeignObjectInstance,
    ) -> Result<Option<ExistentialValueNode>, PackageMetadataError> {
        let checked = self.projection.checked;
        let graph = checked
            .effects
            .metadata_callable_graphs
            .get(&instance.source.graph)
            .ok_or_else(|| invalid("exported object has no checked metadata graph"))?;
        let node = graph
            .graph
            .objects
            .get(instance.source.object as usize)
            .ok_or_else(|| invalid("exported object has no metadata node"))?;
        let object = graph
            .objects
            .get(instance.source.object as usize)
            .ok_or_else(|| invalid("exported object has no typed evidence"))?;
        let environment = instance
            .source
            .environment
            .map(|id| {
                checked
                    .effects
                    .callable_environments
                    .get(&id)
                    .ok_or_else(|| invalid("exported object has no checked lexical environment"))
            })
            .transpose()?;
        let mut generics =
            self.generic_context(environment.map(|environment| environment.owner))?;
        for binding in &node.generic_bindings {
            let parameter = CallableGenericParameter::Graph {
                graph: instance.source.graph,
                scope: binding.scope,
                position: binding.position,
            };
            match environment.and_then(|environment| environment.generics.get(&parameter)) {
                Some(CallableGenericValue::Type(ty)) => {
                    generics.owner_type_names.insert(binding.label.clone());
                    let mut ty = self.projection.metadata_type(*ty)?;
                    normalize_generic_type(&mut ty, &generics.type_names);
                    generics.types.insert(binding.label.clone(), ty);
                }
                Some(
                    CallableGenericValue::EffectRow(row)
                    | CallableGenericValue::EffectRowFromCall { row, .. },
                ) => {
                    generics.rows.insert(
                        binding.label.clone(),
                        self.projection.effect_row_metadata(
                            row,
                            &generics.type_names,
                            &generics.effect_names,
                        )?,
                    );
                }
                None => return Err(invalid("exported object lost its checked generic capture")),
            }
        }
        let parent_scope = self.object_scope(instance, node.parent_scope)?;
        let mut payload_sources = BTreeSet::new();
        for source in &node.payload_sources {
            match source {
                CallableValueSource::Existential(object) => {
                    payload_sources.insert(CallableValueSource::Existential(
                        self.reserve_foreign_object(ForeignObjectInstance {
                            source: etas_effects::MetadataObjectSource {
                                object: *object,
                                ..instance.source
                            },
                            ..instance
                        })?,
                    ));
                }
                CallableValueSource::Value(value) | CallableValueSource::FrozenContract(value) => {
                    let index = self.object_callable(
                        instance,
                        *value,
                        matches!(source, CallableValueSource::FrozenContract(_)),
                    )?;
                    payload_sources.insert(
                        if matches!(source, CallableValueSource::FrozenContract(_)) {
                            CallableValueSource::FrozenContract(index)
                        } else {
                            CallableValueSource::Value(index)
                        },
                    );
                }
                CallableValueSource::Parameter(parameter) => {
                    let key = ParameterCallTarget::Graph {
                        graph: instance.source.graph,
                        parameter: *parameter,
                    };
                    if let Some(origins) =
                        environment.and_then(|environment| environment.parameters.get(&key))
                    {
                        for origin in origins {
                            let Some((source, environment)) = self.checked_origin(*origin)? else {
                                return Ok(None);
                            };
                            let Some(sources) =
                                self.sources(&[source].into(), parent_scope, environment)?
                            else {
                                return Ok(None);
                            };
                            payload_sources.extend(sources);
                        }
                    } else {
                        if parameter.scope == CallableValueScope::Root {
                            return Err(invalid(
                                "exported object payload has an unbound root parameter",
                            ));
                        }
                        payload_sources.insert(CallableValueSource::Parameter(
                            CallableValueParameter {
                                scope: self.object_scope(instance, parameter.scope)?,
                                position: parameter.position,
                            },
                        ));
                    }
                }
            }
        }
        let witnesses = object
            .witnesses
            .iter()
            .map(|witness| {
                let member = match &witness.member {
                    etas_types::SpecMethodIdentity::External { path, .. } => {
                        path.last().cloned().ok_or_else(|| {
                            invalid("exported object member has no declaration identity")
                        })?
                    }
                    _ => {
                        return Err(invalid(
                            "metadata object member has no checked external identity",
                        ));
                    }
                };
                let target = match &witness.target {
                    ExistentialWitnessTarget::Implementation { method, value } => {
                        ExistentialWitnessTarget::Implementation {
                            method: method.clone(),
                            value: self.object_callable(instance, *value, false)?,
                        }
                    }
                    ExistentialWitnessTarget::Bound => ExistentialWitnessTarget::Bound,
                };
                Ok(ExistentialMemberWitness {
                    bound: etas_package_metadata::SpecBound {
                        spec: self.projection.checked_spec_path(&witness.bound.spec)?,
                        args: witness
                            .bound
                            .args
                            .iter()
                            .map(|ty| generics.metadata_type(self.projection, *ty))
                            .collect::<Result<_, _>>()?,
                    },
                    member,
                    target,
                })
            })
            .collect::<Result<_, PackageMetadataError>>()?;
        Ok(Some(ExistentialValueNode {
            parent_scope,
            generic_bindings: generics.bindings.clone(),
            object_type: generics.metadata_type(self.projection, object.object_type)?,
            source_type: generics.metadata_type(self.projection, object.source_type)?,
            payload_sources: payload_sources.into_iter().collect(),
            witnesses,
        }))
    }
}
