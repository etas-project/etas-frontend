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
            .invocation_inputs
            .values()
            .map(|parameters| {
                1 + parameters
                    .values()
                    .map(|sources| 1 + sources.len())
                    .sum::<usize>()
            })
            .sum::<usize>()
        + snapshot
            .parameters
            .values()
            .map(|sources| 1 + sources.len())
            .sum::<usize>()
}

/// Lowering can produce multiple expressions at one source span. Retain their
/// local ordinal so the same site resolves on every round of the solver.
#[derive(Default)]
pub(super) struct CaptureSites {
    keys: std::collections::HashMap<HirExprId, CodeFingerprint>,
    expressions: std::collections::HashMap<CodeFingerprint, HirExprId>,
}

impl EffectSemantics<'_> {
    pub(super) fn callable_invocation_context(id: CallableEnvironmentId) -> CodeFingerprint {
        CodeFingerprint::of_parts(
            "etas.effects.invocation-context.v1",
            [
                id.scope
                    .as_ref()
                    .map_or(&[][..], |scope| scope.as_bytes().as_slice()),
                id.origin.as_bytes().as_slice(),
                id.application.as_bytes().as_slice(),
            ],
        )
    }

    pub(super) fn callable_environment_scope(
        &self,
        owner: etas_hir::HirItemId,
    ) -> Result<CodeFingerprint, EffectSpecializationError> {
        let declaration = self
            .hir
            .items
            .get(owner)
            .ok_or_else(|| invalid("capture owner is missing"))?;
        let path = declaration
            .symbol()
            .and_then(|symbol| self.hir.declaration_path(symbol));
        let scope = serde_json::to_vec(&(path, declaration.span()))
            .map_err(|_| invalid("capture scope has no checked identity"))?;
        Ok(CodeFingerprint::of_bytes(
            "etas.effects.capture-owner-scope.v1",
            &scope,
        ))
    }

    pub(crate) fn callable_environment_site(
        &self,
        expr: HirExprId,
    ) -> Result<CodeFingerprint, EffectSpecializationError> {
        self.capture_site_index()
            .keys
            .get(&expr)
            .copied()
            .ok_or_else(|| invalid("capture site expression is missing"))
    }

    pub(super) fn capture_site_expression(&self, site: CodeFingerprint) -> Option<HirExprId> {
        self.capture_site_index().expressions.get(&site).copied()
    }

    fn capture_site_index(&self) -> &CaptureSites {
        self.type_queries.capture_sites.get_or_init(|| {
            let mut sites = CaptureSites::default();
            let mut ordinals = std::collections::HashMap::<_, u32>::new();
            for (expr, node) in self.hir.exprs.iter() {
                let span = node.span(&self.hir.blocks);
                let ordinal = ordinals.entry(span).or_default();
                let mut bytes = [0u8; 16];
                bytes[..4].copy_from_slice(&span.source.0.to_le_bytes());
                bytes[4..8].copy_from_slice(&span.range.start.0.to_le_bytes());
                bytes[8..12].copy_from_slice(&span.range.end.0.to_le_bytes());
                bytes[12..].copy_from_slice(&ordinal.to_le_bytes());
                let key =
                    CodeFingerprint::of_bytes("etas.effects.callable-capture-site.v2", &bytes);
                sites.keys.insert(expr, key);
                sites.expressions.insert(key, expr);
                *ordinal += 1;
            }
            sites
        })
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
                    && let Some(known) = self.checked_value_sources(expr)
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

    /// One frame joins the contributions of its input equations. Re-evaluating
    /// the same equation refines that contribution, preserving other callers.
    pub(super) fn record_invocation_environment(
        &self,
        id: CallableEnvironmentId,
        inputs: CodeFingerprint,
        mut snapshot: CallableEnvironment,
    ) -> Result<(), EffectSpecializationError> {
        let previous = self.callable_environments.borrow().get(&id).cloned();
        let mut contributions = previous
            .as_ref()
            .map(|previous| previous.invocation_inputs.clone())
            .unwrap_or_default();
        if let Some(previous) = contributions.get(&inputs) {
            for (target, sources) in &mut snapshot.parameters {
                if let Some(previous) = previous.get(target) {
                    self.close_origin_set(previous, sources);
                }
            }
        }
        contributions.insert(inputs, std::mem::take(&mut snapshot.parameters));
        for contribution in contributions.values() {
            for (target, sources) in contribution {
                snapshot
                    .parameters
                    .entry(*target)
                    .or_default()
                    .extend(sources);
            }
        }
        if let Some(previous) = &previous {
            let mut contracts = previous.clone();
            // Restored frames retain their checked aggregate; fresh analysis
            // frames have contributions that can be refined independently.
            if !previous.invocation_inputs.is_empty() {
                contracts.parameters.clear();
            }
            snapshot.join_assign(&contracts).map_err(invalid)?;
        }
        snapshot.invocation_inputs = contributions;
        if previous.as_ref() == Some(&snapshot) {
            return Ok(());
        }
        let nodes = self
            .callable_environment_nodes
            .get()
            .saturating_sub(previous.as_ref().map_or(0, node_count))
            .saturating_add(node_count(&snapshot));
        if nodes > etas_package_metadata::MAX_METADATA_GRAPH_NODES {
            return Err(invalid(
                "callable environment graph exceeds the analysis node budget",
            ));
        }
        Arc::make_mut(&mut self.callable_environments.borrow_mut()).insert(id, snapshot);
        self.callable_environment_nodes.set(nodes);
        self.callable_summaries.borrow_mut().invalidate();
        Ok(())
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
                    let refined_application = !current.is_empty()
                        && current.iter().all(|source| {
                            !matches!(
                                source,
                                LatentSource::Unavailable { .. }
                                    | LatentSource::Parameter { .. }
                                    | LatentSource::Projection(_)
                            )
                        })
                        && self.concrete_application_base(
                            *parameter,
                            joined
                                .parameters
                                .iter()
                                .map(|(target, sources)| (*target, sources)),
                        );
                    if refined_application {
                        sources.retain(|source| {
                            !matches!(
                                source,
                                LatentSource::Unavailable { .. }
                                    | LatentSource::Parameter { .. }
                                    | LatentSource::Projection(_)
                            )
                        });
                    }
                    sources.retain(|source| match source {
                        LatentSource::Unavailable { expr } => self.checked_value_sources(*expr).is_none()
                            || current.is_empty()
                            || current.iter().any(|source| matches!(source, LatentSource::Unavailable { expr: missing } if missing == expr)),
                        _ => true,
                    });
                }
            }
            joined.join_assign(&previous).map_err(|reason| {
                invalid(&format!(
                    "{reason}: environment {id:?}, existing owner {:?}, incoming owner {:?}",
                    previous.owner, joined.owner
                ))
            })?;
            for (parameter, sources) in &mut joined.parameters {
                if let Some(previous) = previous.parameters.get(parameter) {
                    self.close_origin_set(previous, sources);
                }
            }
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
                generic_context: None,
                instance: id.instance,
                scope: None,
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
                generic_context: None,
                instance: Some(id.lexical_instance()),
                scope: None,
                // Retain the first lexical creation/application coordinate.
                // Repeated returns rebind the frame instead of hashing a chain.
                origin: id.origin,
                application,
            }
        })
    }

    /// Select checked zero-argument return positions without turning the
    /// captured callable into a symbolic parameter or executing its effects.
    pub(super) fn descriptor_projected_origins(
        &self,
        sources: &BTreeSet<LatentSource>,
        path: &[etas_code::descriptor::ValueProjectionStep],
        state: &EffectState,
    ) -> Option<BTreeSet<LatentSource>> {
        if let Some(inputs) = etas_code::descriptor::ValueProjectionStep::call_inputs(path).first()
        {
            let application = self.native_call_inputs(*inputs)?.call;
            return self.project_value_origins(sources, path, state, application);
        }
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
            let selected = super::return_projection::returned_positions(
                exits
                    .value_projections
                    .iter()
                    .map(|(path, sources)| (path.as_slice(), sources)),
                rest,
            );
            let selected = if selected.is_empty() {
                vec![(exits.callable_return_sources()?, rest.to_vec())]
            } else {
                let mut positions = Vec::new();
                for (sources, suffix) in selected {
                    let crate::ProjectedReturnSources::Known(sources) = sources else {
                        return None;
                    };
                    positions.push((sources.clone(), suffix));
                }
                positions
            };
            let snapshot = environment
                .map(|id| self.callable_environment(id))
                .transpose()
                .ok()?;
            for (returned, suffix) in selected {
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
                                origins.extend(self.descriptor_projected_origins(
                                    roots,
                                    &projection.path,
                                    state,
                                )?);
                            }
                        }
                        R::Reference(_) => return None,
                    }
                }
                result.extend(self.descriptor_projected_origins(&origins, &suffix, state)?);
            }
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
        // Scope is a separate coordinate: restoring a frame repeatedly does
        // not manufacture a history of hashed application sites.
        let scope = self.callable_environment_scope(owner)?;
        // Resolve each checked frame's complete generic context before naming
        // its edges. Different instantiations never occupy one identity slot.
        let mut pending = vec![root];
        let mut originals = BTreeMap::new();
        let mut identities = BTreeMap::new();
        let mut generic_contexts = BTreeMap::new();
        while let Some(source) = pending.pop() {
            if originals.contains_key(&source) {
                continue;
            }
            let original = self.callable_environment(source)?;
            for origin in original.parameters.values().flatten() {
                let child = match origin {
                    LatentSource::Metadata(source) => source.environment,
                    LatentSource::MetadataObject(source) => source.environment,
                    LatentSource::Existential { environment, .. }
                    | LatentSource::Captured { environment, .. }
                    | LatentSource::SpecMethod { environment, .. }
                    | LatentSource::MetadataSpecMethod { environment, .. } => Some(*environment),
                    LatentSource::FrozenContract { environment, .. }
                    | LatentSource::Reference { environment, .. }
                    | LatentSource::StdReference { environment, .. } => *environment,
                    _ => None,
                };
                pending.extend(child);
            }
            let generics =
                self.substitute_generic_values(original.generics.clone(), arguments, state.owner)?;
            let mut id = mapped(source);
            if id.instance.is_none() && source.scope != Some(scope) {
                id.instance = Some(source.lexical_instance());
            }
            id.scope = Some(scope);
            id.generic_context = CallableEnvironment::generic_identity(&generics);
            identities.insert(source, id);
            generic_contexts.insert(source, generics);
            originals.insert(source, original);
        }
        let mapped = |id| identities[&id];
        for (id, original) in originals {
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
                                            self.descriptor_projected_origins(sources, path, state).ok_or_else(|| {
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
                            object.environment = object.environment.map(mapped);
                            resolved.insert(LatentSource::MetadataObject(object));
                        }
                        LatentSource::Existential { pack, environment } => {
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
                                source.environment = Some(mapped(parent));
                            }
                            resolved.insert(LatentSource::Metadata(source));
                        }
                        LatentSource::Captured { unit, environment } => {
                            resolved.insert(LatentSource::Captured {
                                unit,
                                environment: mapped(environment),
                            });
                        }
                        LatentSource::SpecMethod { call, environment } => {
                            resolved.insert(LatentSource::SpecMethod {
                                call,
                                environment: mapped(environment),
                            });
                        }
                        LatentSource::MetadataSpecMethod {
                            invocation,
                            environment,
                        } => {
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
                            resolved.insert(LatentSource::Reference {
                                unit,
                                value,
                                environment: environment.map(mapped),
                            });
                        }
                        LatentSource::StdReference { value, environment } => {
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
                    invocation_origin: original.invocation_origin,
                    invocation_inputs: Default::default(),
                    owner,
                    parameters,
                    generics: generic_contexts
                        .remove(&id)
                        .expect("prepared capture generics"),
                },
            )?;
        }
        Ok(mapped(root))
    }
}

#[cfg(test)]
mod tests;
