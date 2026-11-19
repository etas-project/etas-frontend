use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use etas_code::CodeFingerprint;
use etas_hir::HirExprId;

use super::{
    engine::EffectSemantics,
    specialize::{EffectSpecializationError, TraceArgumentBindings},
    state::{EffectState, LatentSource},
};
use crate::{
    ParameterCallTarget,
    facts::{CallableEnvironment, CallableEnvironmentId},
};

fn invalid(reason: &str) -> EffectSpecializationError {
    EffectSpecializationError::InvalidCallableEnvironment {
        reason: reason.into(),
    }
}

fn node_count(snapshot: &CallableEnvironment) -> usize {
    1 + snapshot.generics.len()
        + snapshot
            .parameters
            .values()
            .map(|sources| 1 + sources.len())
            .sum::<usize>()
}

impl EffectSemantics<'_> {
    pub(crate) fn callable_environment_site(
        &self,
        expr: HirExprId,
    ) -> Result<CodeFingerprint, EffectSpecializationError> {
        let span = self
            .hir
            .exprs
            .get(expr)
            .ok_or_else(|| invalid("capture site expression is missing"))?
            .span(&self.hir.blocks);
        let mut bytes = Vec::with_capacity(12);
        bytes.extend_from_slice(&span.source.0.to_le_bytes());
        bytes.extend_from_slice(&span.range.start.0.to_le_bytes());
        bytes.extend_from_slice(&span.range.end.0.to_le_bytes());
        Ok(CodeFingerprint::of_bytes(
            "etas.effects.callable-capture-site.v1",
            &bytes,
        ))
    }

    pub(crate) fn callable_environment(
        &self,
        id: CallableEnvironmentId,
    ) -> Result<CallableEnvironment, EffectSpecializationError> {
        if self.callable_environment_nodes.get() > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
            return Err(invalid(
                "callable environment graph exceeds the analysis node budget",
            ));
        }
        let mut snapshot = self
            .callable_environments
            .borrow()
            .get(&id)
            .cloned()
            .ok_or_else(|| invalid("checked callable environment is missing"))?;
        let mut nodes = 1 + snapshot.generics.len();
        for sources in snapshot.parameters.values_mut() {
            let mut resolved = BTreeSet::new();
            for source in sources.iter().copied() {
                if let LatentSource::Unavailable { expr } = source
                    && self.context.view().index().expr_item.get(&expr) == Some(&snapshot.owner)
                    && let Some(known) = self.call_results.value_sources(expr)
                {
                    if nodes + resolved.len() + known.len()
                        > etas_package_metadata::MAX_METADATA_GRAPH_NODES
                    {
                        return Err(invalid(
                            "resolved callable environment exceeds the analysis node budget",
                        ));
                    }
                    resolved.extend(known);
                } else {
                    resolved.insert(source);
                }
            }
            nodes += 1 + resolved.len();
            if nodes > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
                return Err(invalid(
                    "resolved callable environment exceeds the analysis node budget",
                ));
            }
            *sources = resolved;
        }
        Ok(snapshot)
    }

    pub(crate) fn record_callable_environment(
        &self,
        id: CallableEnvironmentId,
        snapshot: CallableEnvironment,
    ) -> Result<(), EffectSpecializationError> {
        let mut environments = self.callable_environments.borrow_mut();
        let existing = environments.get(&id);
        let old_nodes = existing.map_or(0, node_count);
        let mut joined = snapshot;
        if let Some(existing) = existing {
            let mut previous = existing.clone();
            // Refine only the same checked call's pending origin after a later
            // transfer has supplied its complete, scoped return-source evidence.
            for (parameter, sources) in &mut previous.parameters {
                if let Some(current) = joined.parameters.get(parameter) {
                    sources.retain(|source| match source {
                        LatentSource::Unavailable { expr } => self.call_results.value_sources(*expr).is_none()
                            || current.is_empty()
                            || current.iter().any(|source| matches!(source, LatentSource::Unavailable { expr: missing } if missing == expr)),
                        _ => true,
                    });
                }
            }
            joined.join_assign(&previous).map_err(invalid)?;
        }
        let nodes = self
            .callable_environment_nodes
            .get()
            .saturating_sub(old_nodes)
            .saturating_add(node_count(&joined));
        if nodes > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
            return Err(invalid(
                "callable environment graph exceeds the analysis node budget",
            ));
        }
        if existing != Some(&joined) {
            Arc::make_mut(&mut environments).insert(id, joined);
            self.callable_environment_nodes.set(nodes);
            self.callable_summaries.borrow_mut().invalidate();
        }
        Ok(())
    }

    pub(crate) fn instantiate_callable_environment(
        &self,
        root: CallableEnvironmentId,
        application: HirExprId,
        bindings: &TraceArgumentBindings,
        state: &EffectState,
    ) -> Result<CallableEnvironmentId, EffectSpecializationError> {
        let (types, rows) = self.checked_call_generic_values(application, state)?;
        let parameters = self
            .types
            .facts
            .checked_method_calls
            .get(&application)
            .into_iter()
            .flat_map(|fact| fact.owner_bindings.iter().copied())
            .collect();
        self.instantiate_callable_environment_with_values(
            root,
            self.callable_environment_site(application)?,
            bindings,
            state,
            super::callable_generics::ScopedGenericArguments {
                types: &types,
                parameters: Some(&parameters),
                rows: &rows,
            },
        )
    }

    pub(super) fn instantiate_callable_environment_with_values(
        &self,
        root: CallableEnvironmentId,
        application: CodeFingerprint,
        bindings: &TraceArgumentBindings,
        state: &EffectState,
        arguments: super::callable_generics::ScopedGenericArguments<'_>,
    ) -> Result<CallableEnvironmentId, EffectSpecializationError> {
        self.instantiate_callable_environment_mapped(root, bindings, state, arguments, |id| {
            CallableEnvironmentId {
                origin: id.origin,
                application,
            }
        })
    }

    pub(super) fn instantiate_returned_callable_environment(
        &self,
        root: CallableEnvironmentId,
        application: CodeFingerprint,
        bindings: &TraceArgumentBindings,
        state: &EffectState,
        arguments: super::callable_generics::ScopedGenericArguments<'_>,
    ) -> Result<CallableEnvironmentId, EffectSpecializationError> {
        self.instantiate_callable_environment_mapped(root, bindings, state, arguments, |id| {
            CallableEnvironmentId {
                // Retain the nested creation site, while all recursive returns use
                // the current instantiated caller context instead of growing a chain.
                origin: CodeFingerprint::of_parts(
                    "etas.effects.returned-capture-origin.v1",
                    [
                        id.origin.as_bytes().as_slice(),
                        id.application.as_bytes().as_slice(),
                    ],
                ),
                application,
            }
        })
    }

    /// Select checked zero-argument return positions without turning the
    /// captured callable into a symbolic parameter or executing its effects.
    fn descriptor_projected_origins(
        &self,
        sources: &BTreeSet<LatentSource>,
        path: &[etas_code::descriptor::ValueProjectionStep],
    ) -> Option<BTreeSet<LatentSource>> {
        if let Some(origins) = self.project_parameter_origins(sources.iter().copied(), path) {
            return Some(origins);
        }
        let [
            etas_code::descriptor::ValueProjectionStep::CallResult,
            rest @ ..,
        ] = path
        else {
            return None;
        };
        let mut result = BTreeSet::new();
        for source in sources {
            let (unit, environment) = match source {
                LatentSource::Captured { unit, environment } => (*unit, Some(*environment)),
                LatentSource::Unit(unit) => (*unit, None),
                _ => return None,
            };
            if !self.invocation_params(unit, environment).ok()??.is_empty() {
                return None;
            }
            let crate::EffectUnit::AnonymousFlow { value, .. } = unit else {
                return None;
            };
            let span = self.hir.exprs.get(value)?.span(&self.hir.blocks);
            let summary = self
                .solve_callable_summary_in_mode(
                    *source,
                    span,
                    super::callable_summary::CallableSummaryMode::Descriptor,
                )
                .ok()?;
            let exits = summary.exits.as_ref()?;
            let selected = exits
                .value_projections
                .iter()
                .filter(|(prefix, _)| rest.starts_with(prefix))
                .max_by_key(|(prefix, _)| prefix.len());
            let (returned, suffix) = match selected {
                Some((prefix, crate::ProjectedReturnSources::Known(returned))) => {
                    (returned.clone(), &rest[prefix.len()..])
                }
                Some((_, crate::ProjectedReturnSources::Unavailable)) => return None,
                None => {
                    // An aggregate projection selects all of its callable
                    // leaf positions, excluding unrelated sibling payloads.
                    let mut descendants = exits
                        .value_projections
                        .iter()
                        .filter(|(position, _)| position.starts_with(rest))
                        .peekable();
                    if descendants.peek().is_some() {
                        let mut returned = BTreeSet::new();
                        for (_, position) in descendants {
                            let crate::ProjectedReturnSources::Known(found) = position else {
                                return None;
                            };
                            returned.extend(found.iter().cloned());
                        }
                        (returned, &rest[rest.len()..])
                    } else {
                        (exits.callable_return_sources()?, rest)
                    }
                }
            };
            let snapshot = environment
                .map(|id| self.callable_environment(id))
                .transpose()
                .ok()?;
            let mut origins = BTreeSet::new();
            for returned in returned {
                use crate::CallableReturnSource as R;
                match returned {
                    R::Captured { unit, environment } => {
                        origins.insert(LatentSource::Captured { unit, environment });
                    }
                    R::AnonymousFlow(unit) => {
                        origins.insert(match environment {
                            Some(environment) => LatentSource::Captured { unit, environment },
                            None => LatentSource::Unit(unit),
                        });
                    }
                    R::SourceItem(item) => {
                        origins.insert(LatentSource::Unit(crate::EffectUnit::Item(item)));
                    }
                    R::FrozenContract {
                        unit,
                        reference,
                        environment,
                    } => {
                        origins.insert(LatentSource::FrozenContract {
                            unit,
                            reference,
                            environment,
                        });
                    }
                    R::Metadata(source) => {
                        origins.insert(LatentSource::Metadata(source));
                    }
                    R::MetadataObject(source) => {
                        origins.insert(LatentSource::MetadataObject(source));
                    }
                    R::Existential { pack, environment } => {
                        origins.insert(LatentSource::Existential { pack, environment });
                    }
                    R::ExternalFrozen { reflection } => {
                        origins.insert(LatentSource::ExternalFrozen { reflection });
                    }
                    R::ExternalReference { value, frozen } => {
                        origins.insert(LatentSource::ExternalReference { value, frozen });
                    }
                    R::Parameter(symbol) => {
                        origins.extend(
                            snapshot
                                .as_ref()?
                                .parameters
                                .get(&ParameterCallTarget::Native(symbol))?
                                .iter()
                                .copied(),
                        );
                    }
                    R::Projection(id) => {
                        if let Some(bound) = snapshot.as_ref().and_then(|snapshot| {
                            snapshot
                                .parameters
                                .get(&ParameterCallTarget::Projection(id))
                        }) {
                            origins.extend(bound.iter().copied());
                        } else {
                            let projection = self.parameter_projection(id)?;
                            let roots = snapshot
                                .as_ref()?
                                .parameters
                                .get(&ParameterCallTarget::Native(projection.parameter))?;
                            origins.extend(
                                self.descriptor_projected_origins(roots, &projection.path)?,
                            );
                        }
                    }
                    R::Reference(_) => return None,
                }
            }
            result.extend(self.descriptor_projected_origins(&origins, suffix)?);
        }
        Some(result)
    }

    fn instantiate_callable_environment_mapped(
        &self,
        root: CallableEnvironmentId,
        bindings: &TraceArgumentBindings,
        state: &EffectState,
        arguments: super::callable_generics::ScopedGenericArguments<'_>,
        mapped: impl Fn(CallableEnvironmentId) -> CallableEnvironmentId + Copy,
    ) -> Result<CallableEnvironmentId, EffectSpecializationError> {
        let owner = state
            .owner
            .ok_or_else(|| invalid("capture instantiation has no checked owner"))?;
        let mut pending = vec![root];
        let mut visited = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            let original = self.callable_environment(id)?;
            let mut parameters = BTreeMap::new();
            for (parameter, sources) in original.parameters {
                let mut resolved = BTreeSet::new();
                for source in sources {
                    match source {
                        LatentSource::Projection(id) => {
                            let target = ParameterCallTarget::Projection(id);
                            let root = self
                                .parameter_projection_target(target)
                                .map(|(root, _)| root);
                            if bindings.contains_key(&target)
                                || root.is_some_and(|root| bindings.contains_key(&root))
                            {
                                if let Some(binding) = self.trace_argument(
                                    target,
                                    "captured value projection",
                                    bindings,
                                    state,
                                )? {
                                    let origins = match &binding {
                                        super::specialize::TraceArgumentBinding::ProjectedOrigins { sources, path } =>
                                            self.descriptor_projected_origins(sources, path).ok_or_else(|| {
                                                self.callable_summaries.borrow().interruption.clone()
                                                    .map(EffectSpecializationError::Interrupted)
                                                    .unwrap_or_else(|| {
                                                        let projections = sources.iter().filter_map(|source| match source {
                                                            LatentSource::Projection(id) => self.parameter_projection(*id).map(|projection| projection.path),
                                                            _ => None,
                                                        }).collect::<Vec<_>>();
                                                        invalid(&format!("captured return projection {path:?} of {sources:?} (paths {projections:?}) has no checked callable origins"))
                                                    })
                                            })?,
                                        _ => self.trace_argument_origins(&binding, state)?,
                                    };
                                    resolved.extend(origins);
                                } else {
                                    resolved.insert(source);
                                }
                            } else {
                                resolved.insert(source);
                            }
                        }

                        LatentSource::MetadataObject(mut object) => {
                            pending.extend(object.environment);
                            object.environment = object.environment.map(mapped);
                            resolved.insert(LatentSource::MetadataObject(object));
                        }
                        LatentSource::Existential { pack, environment } => {
                            pending.push(environment);
                            resolved.insert(LatentSource::Existential {
                                pack,
                                environment: mapped(environment),
                            });
                        }
                        LatentSource::FrozenContract {
                            unit,
                            reference,
                            environment,
                        } => {
                            pending.extend(environment);
                            resolved.insert(LatentSource::FrozenContract {
                                unit,
                                reference,
                                environment: environment.map(mapped),
                            });
                        }
                        LatentSource::Parameter { symbol, .. }
                            if bindings.contains_key(&ParameterCallTarget::Native(symbol)) =>
                        {
                            resolved.extend(self.trace_argument_origins(
                                &bindings[&ParameterCallTarget::Native(symbol)],
                                state,
                            )?);
                        }
                        LatentSource::Metadata(mut source) => {
                            if let Some(parent) = source.environment {
                                pending.push(parent);
                                source.environment = Some(mapped(parent));
                            }
                            resolved.insert(LatentSource::Metadata(source));
                        }
                        LatentSource::Captured { unit, environment } => {
                            pending.push(environment);
                            resolved.insert(LatentSource::Captured {
                                unit,
                                environment: mapped(environment),
                            });
                        }
                        LatentSource::SpecMethod { call, environment } => {
                            pending.push(environment);
                            resolved.insert(LatentSource::SpecMethod {
                                call,
                                environment: mapped(environment),
                            });
                        }
                        LatentSource::MetadataSpecMethod {
                            invocation,
                            environment,
                        } => {
                            pending.push(environment);
                            resolved.insert(LatentSource::MetadataSpecMethod {
                                invocation,
                                environment: mapped(environment),
                            });
                        }
                        LatentSource::Reference {
                            unit,
                            value,
                            environment,
                        } => {
                            pending.extend(environment);
                            resolved.insert(LatentSource::Reference {
                                unit,
                                value,
                                environment: environment.map(mapped),
                            });
                        }
                        LatentSource::StdReference { value, environment } => {
                            pending.extend(environment);
                            resolved.insert(LatentSource::StdReference {
                                value,
                                environment: environment.map(mapped),
                            });
                        }
                        other => {
                            resolved.insert(other);
                        }
                    }
                }
                parameters.insert(parameter, resolved);
            }
            self.record_callable_environment(
                mapped(id),
                CallableEnvironment {
                    owner,
                    parameters,
                    generics: self.substitute_generic_values(
                        original.generics,
                        arguments,
                        state.owner,
                    )?,
                },
            )?;
        }
        Ok(mapped(root))
    }
}

#[cfg(test)]
mod tests;
