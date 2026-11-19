use super::*;
use etas_effects::facts::{CallableEnvironmentId, CallableValueOrigin};

impl Builder<'_, '_> {
    pub(super) fn return_projections(
        &mut self,
        summary: &EffectSummary,
        scope: CallableValueScope,
        environment: Option<CallableEnvironmentId>,
    ) -> Result<Vec<etas_package_metadata::CallableReturnedProjection>, PackageMetadataError> {
        let mut result = Vec::new();
        if let Some(exits) = &summary.exits {
            for (path, sources) in &exits.value_projections {
                result.push(etas_package_metadata::CallableReturnedProjection {
                    path: path.clone(),
                    sources: match sources {
                        etas_effects::ProjectedReturnSources::Known(sources) => {
                            self.sources(sources, scope, environment)?
                        }
                        etas_effects::ProjectedReturnSources::Unavailable => None,
                    },
                });
            }
        }
        Ok(result)
    }

    pub(super) fn sources(
        &mut self,
        sources: &BTreeSet<CallableReturnSource>,
        scope: CallableValueScope,
        environment: Option<CallableEnvironmentId>,
    ) -> Result<Option<Vec<CallableValueSource>>, PackageMetadataError> {
        let mut pending = sources
            .iter()
            .cloned()
            .map(|source| (source, environment))
            .collect::<Vec<_>>();
        let mut output = BTreeSet::new();
        let mut visited = BTreeSet::new();
        while let Some((source, environment)) = pending.pop() {
            if !visited.insert((source.clone(), environment)) {
                continue;
            }
            if visited.len() > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
                return Err(invalid(
                    "callable return sources exceed the metadata node budget",
                ));
            }
            let instance = match source {
                CallableReturnSource::Projection(id) => {
                    if let Some(environment) = environment {
                        let snapshot = self
                            .projection
                            .checked
                            .effects
                            .callable_environments
                            .get(&environment)
                            .ok_or_else(|| {
                                invalid("returned projection has no checked capture environment")
                            })?;
                        if let Some(origins) = snapshot
                            .parameters
                            .get(&etas_effects::ParameterCallTarget::Projection(id))
                        {
                            for origin in origins {
                                let Some((source, environment)) = self.checked_origin(*origin)?
                                else {
                                    return Ok(None);
                                };
                                pending.push((source, environment));
                            }
                            continue;
                        }
                        let projection = self
                            .projection
                            .checked
                            .effects
                            .parameter_projections
                            .get(&id)
                            .ok_or_else(|| {
                                invalid("returned projection has no checked source path")
                            })?;
                        if let Some(origins) =
                            snapshot
                                .parameters
                                .get(&etas_effects::ParameterCallTarget::Native(
                                    projection.parameter,
                                ))
                        {
                            // A recursive factory can bind the captured container
                            // without separately binding every projected element.
                            // Keep that checked path on the caller's actual parameter.
                            let projected = origins
                                .iter()
                                .map(|origin| {
                                    let target = match origin {
                                        CallableValueOrigin::Parameter { symbol, .. } => {
                                            etas_effects::ParameterCallTarget::Native(*symbol)
                                        }
                                        CallableValueOrigin::Projection(id) => {
                                            etas_effects::ParameterCallTarget::Projection(*id)
                                        }
                                        _ => return Ok(None),
                                    };
                                    self.target_parameter(target, scope)?
                                        .map(|root| self.with_projection(root, &projection.path))
                                        .transpose()
                                })
                                .collect::<Result<Option<Vec<_>>, PackageMetadataError>>()?;
                            if let Some(projected) = projected {
                                output.extend(
                                    projected.into_iter().map(CallableValueSource::Parameter),
                                );
                                continue;
                            }
                        }
                    }
                    let Some(parameter) = self.target_parameter(
                        etas_effects::ParameterCallTarget::Projection(id),
                        scope,
                    )?
                    else {
                        if self.method_root {
                            let fact = self
                                .projection
                                .checked
                                .effects
                                .parameter_projections
                                .get(&id);
                            return Err(invalid(&format!(
                                "returned projection {id:?} has no lexical parameter in {scope:?}: {fact:?}; capture {environment:?}"
                            )));
                        }
                        return Ok(None);
                    };
                    output.insert(CallableValueSource::Parameter(parameter));
                    continue;
                }

                CallableReturnSource::MetadataObject(source) => {
                    output.insert(CallableValueSource::Existential(
                        self.reserve_foreign_object(ForeignObjectInstance {
                            source,
                            root: scope,
                        })?,
                    ));
                    continue;
                }
                CallableReturnSource::Existential { pack, environment } => {
                    output.insert(CallableValueSource::Existential(self.reserve_object(
                        NativeObjectInstance {
                            pack,
                            environment,
                            scope,
                        },
                    )?));
                    continue;
                }
                CallableReturnSource::ExternalFrozen { reflection } => {
                    let reference = self.external_reference(reflection, true)?;
                    output.insert(CallableValueSource::FrozenContract(
                        self.reserve_reference(reference, scope)?,
                    ));
                    continue;
                }
                CallableReturnSource::ExternalReference { value, frozen } => {
                    let reference = self.external_reference(value, frozen)?;
                    let index = self.reserve_reference(reference, scope)?;
                    output.insert(if frozen {
                        CallableValueSource::FrozenContract(index)
                    } else {
                        CallableValueSource::Value(index)
                    });
                    continue;
                }
                CallableReturnSource::FrozenContract {
                    unit,
                    reference,
                    environment,
                } => {
                    let index = if let Some(value) = reference {
                        let mut references = self
                            .projection
                            .checked
                            .effects
                            .callable_references
                            .keys()
                            .filter(|reference| {
                                reference.value == value && reference.environment.is_none()
                            });
                        let reference = match (references.next(), references.next()) {
                            (Some(reference), None) => {
                                let mut reference = reference.clone();
                                reference.environment = environment;
                                reference
                            }
                            _ => {
                                return Err(invalid(
                                    "frozen contract reference has no unique checked callable fact",
                                ));
                            }
                        };
                        self.reserve_reference(reference, scope)?
                    } else {
                        self.reserve(Instance { unit, environment }, scope)?
                    };
                    output.insert(CallableValueSource::FrozenContract(index));
                    continue;
                }
                CallableReturnSource::Parameter(symbol) => {
                    if let Some(environment) = environment {
                        let snapshot = self
                            .projection
                            .checked
                            .effects
                            .callable_environments
                            .get(&environment)
                            .ok_or_else(|| {
                                invalid("returned capture has no checked environment")
                            })?;
                        if let Some(origins) = snapshot
                            .parameters
                            .get(&etas_effects::ParameterCallTarget::Native(symbol))
                        {
                            // An instantiated scalar has checked empty callable
                            // origins; it does not make the remaining return graph unknown.
                            if origins.is_empty() {
                                continue;
                            }
                            for origin in origins {
                                let Some((source, environment)) = self.checked_origin(*origin)?
                                else {
                                    return Ok(None);
                                };
                                pending.push((source, environment));
                            }
                            continue;
                        }
                    }
                    let Some(parameter) = self.parameter(symbol, scope)? else {
                        if self.method_root {
                            let symbol = self.projection.checked.hir.symbols.get(symbol);
                            return Err(invalid(&format!(
                                "returned parameter has no lexical binding in {scope:?}: {symbol:?}; capture {environment:?}"
                            )));
                        }
                        return Ok(None);
                    };
                    if !self.scope_contains(scope, parameter.scope) {
                        if self.method_root {
                            return Err(invalid(&format!(
                                "returned parameter {parameter:?} is outside {scope:?}"
                            )));
                        }
                        return Ok(None);
                    }
                    output.insert(CallableValueSource::Parameter(parameter));
                    continue;
                }
                CallableReturnSource::SourceItem(item) => Instance {
                    unit: EffectUnit::Item(item),
                    environment: None,
                },
                CallableReturnSource::AnonymousFlow(unit) => Instance { unit, environment },
                CallableReturnSource::Captured { unit, environment } => Instance {
                    unit,
                    environment: Some(environment),
                },
                CallableReturnSource::Metadata(source) => {
                    let index = self.reserve_foreign(ForeignInstance {
                        source,
                        root: scope,
                    })?;
                    output.insert(
                        if source.analysis == etas_effects::CallableAnalysisKind::FrozenContract {
                            CallableValueSource::FrozenContract(index)
                        } else {
                            CallableValueSource::Value(index)
                        },
                    );
                    continue;
                }
                CallableReturnSource::Reference(mut reference) => {
                    reference.environment = reference.environment.or(environment);
                    output.insert(CallableValueSource::Value(
                        self.reserve_reference(reference, scope)?,
                    ));
                    continue;
                }
            };
            let index = self.reserve(instance, scope)?;
            output.insert(CallableValueSource::Value(index));
        }
        Ok(Some(output.into_iter().collect()))
    }

    pub(super) fn reserve(
        &mut self,
        instance: Instance,
        scope: CallableValueScope,
    ) -> Result<u32, PackageMetadataError> {
        if let Some(index) = self.units.get(&instance) {
            return Ok(*index);
        }
        if self.values.len() >= etas_package_metadata::MAX_METADATA_GRAPH_NODES {
            return Err(invalid("callable value graph exceeds metadata node budget"));
        }
        let index = u32::try_from(self.values.len())
            .map_err(|_| invalid("callable value graph exceeds u32"))?;
        self.units.insert(instance, index);
        self.parents.insert(
            index,
            if instance.environment.is_some()
                || matches!(instance.unit, EffectUnit::AnonymousFlow { .. })
            {
                // Instantiated captures can depend on the caller's parameters,
                // including when the captured callable is a named source flow.
                scope
            } else {
                CallableValueScope::Root
            },
        );
        self.values.push(None);
        self.origins
            .push(crate::package_metadata::CallableGraphOrigin::Native {
                unit: instance.unit,
                environment: instance.environment,
            });
        self.pending.push(Pending::Native(index, instance));
        Ok(index)
    }
}

impl Builder<'_, '_> {
    fn external_reference(
        &self,
        value: etas_hir::HirExprId,
        frozen: bool,
    ) -> Result<etas_effects::CallableReturnReference, PackageMetadataError> {
        let mut matches = self.projection.checked.effects.callable_references.keys().filter(|reference| {
            reference.value == value && reference.environment.is_none()
                && matches!(&reference.target, etas_effects::CallableReferenceTarget::External { frozen: actual, .. } if *actual == frozen)
        });
        match (matches.next(), matches.next()) {
            (Some(reference), None) => Ok(reference.clone()),
            _ => Err(invalid(
                "external callable has no unique checked reference analysis",
            )),
        }
    }
    pub(super) fn checked_origin(
        &self,
        origin: CallableValueOrigin,
    ) -> Result<Option<(CallableReturnSource, Option<CallableEnvironmentId>)>, PackageMetadataError>
    {
        Ok(Some(match origin {
            CallableValueOrigin::MetadataObject(source) => (
                CallableReturnSource::MetadataObject(source),
                source.environment,
            ),
            CallableValueOrigin::Existential { pack, environment } => (
                CallableReturnSource::Existential { pack, environment },
                Some(environment),
            ),
            CallableValueOrigin::ExternalFrozen { reflection } => {
                (CallableReturnSource::ExternalFrozen { reflection }, None)
            }
            CallableValueOrigin::ExternalReference { value, frozen } => (
                CallableReturnSource::ExternalReference { value, frozen },
                None,
            ),
            CallableValueOrigin::FrozenContract {
                unit,
                reference,
                environment,
            } => (
                CallableReturnSource::FrozenContract {
                    unit,
                    reference,
                    environment,
                },
                environment,
            ),
            CallableValueOrigin::Projection(id) => (CallableReturnSource::Projection(id), None),
            CallableValueOrigin::Parameter { symbol, .. } => {
                (CallableReturnSource::Parameter(symbol), None)
            }
            CallableValueOrigin::Unit(EffectUnit::Item(item)) => {
                (CallableReturnSource::SourceItem(item), None)
            }
            CallableValueOrigin::Unit(unit @ EffectUnit::AnonymousFlow { .. }) => {
                (CallableReturnSource::AnonymousFlow(unit), None)
            }
            CallableValueOrigin::Captured { unit, environment } => (
                CallableReturnSource::Captured { unit, environment },
                Some(environment),
            ),
            CallableValueOrigin::Metadata(source) => (CallableReturnSource::Metadata(source), None),
            CallableValueOrigin::Reference {
                value, environment, ..
            }
            | CallableValueOrigin::StdReference { value, environment } => {
                let mut references = self
                    .projection
                    .checked
                    .effects
                    .callable_references
                    .keys()
                    .filter(|reference| {
                        reference.value == value && reference.environment.is_none()
                    });
                let reference = match (references.next(), references.next()) {
                    (Some(reference), None) => reference.clone(),
                    _ => {
                        return Err(invalid(
                            "captured reference has no unique checked declaration analysis",
                        ));
                    }
                };
                (CallableReturnSource::Reference(reference), environment)
            }
            _ => return Ok(None),
        }))
    }
}
