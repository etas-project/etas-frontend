use super::*;
use etas_effects::facts::{CallableEnvironmentId, CallableValueOrigin};

impl Builder<'_, '_> {
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
                            if origins.is_empty() {
                                return Ok(None);
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
                        return Ok(None);
                    };
                    if !self.scope_contains(scope, parameter.scope) {
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
                    output.insert(CallableValueSource::Value(self.reserve_foreign(
                        ForeignInstance {
                            source,
                            root: scope,
                        },
                    )?));
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

    fn reserve(
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
            if matches!(instance.unit, EffectUnit::AnonymousFlow { .. }) {
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
    pub(super) fn checked_origin(
        &self,
        origin: CallableValueOrigin,
    ) -> Result<Option<(CallableReturnSource, Option<CallableEnvironmentId>)>, PackageMetadataError>
    {
        Ok(Some(match origin {
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
