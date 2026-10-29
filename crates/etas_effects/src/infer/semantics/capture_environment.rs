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
        self.instantiate_callable_environment_with_values(
            root,
            self.callable_environment_site(application)?,
            bindings,
            state,
            &types,
            &rows,
        )
    }

    pub(crate) fn instantiate_callable_environment_with_values(
        &self,
        root: CallableEnvironmentId,
        application: CodeFingerprint,
        bindings: &TraceArgumentBindings,
        state: &EffectState,
        types: &[(String, etas_types::TypeId)],
        rows: &[(String, crate::EffectRow)],
    ) -> Result<CallableEnvironmentId, EffectSpecializationError> {
        let mapped = |id: CallableEnvironmentId| CallableEnvironmentId {
            origin: id.origin,
            application,
        };
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
                        types,
                        rows,
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
