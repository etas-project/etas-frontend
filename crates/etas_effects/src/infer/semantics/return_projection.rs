//! Select a callable's returned payload before binding the caller's arguments.
use std::collections::BTreeSet;

use etas_code::descriptor::ValueProjectionStep as Step;
use etas_hir::HirExprId;

use super::{
    engine::EffectSemantics,
    specialize::{TraceArgumentBinding, TraceArgumentBindings},
    state::{EffectState, LatentSource},
};
use crate::{CallableReturnSource, EffectUnit, ProjectedReturnSources};

impl EffectSemantics<'_> {
    pub(super) fn trace_argument_origins_at(
        &self,
        binding: &TraceArgumentBinding,
        state: &EffectState,
        application: HirExprId,
    ) -> Result<BTreeSet<LatentSource>, super::specialize::EffectSpecializationError> {
        if let TraceArgumentBinding::ProjectedOrigins { sources, path } = binding {
            return Ok(self
                .project_value_origins(sources, path, state, application)
                .unwrap_or_else(|| [LatentSource::Unavailable { expr: application }].into()));
        }
        self.trace_argument_origins(binding, state)
    }

    pub(super) fn projected_return_bindings(
        &self,
        returned: &BTreeSet<CallableReturnSource>,
        suffix: &[Step],
        bindings: &TraceArgumentBindings,
        state: &EffectState,
        application: HirExprId,
        environment: Option<crate::facts::CallableEnvironmentId>,
    ) -> Option<BTreeSet<LatentSource>> {
        if suffix.is_empty() {
            return self
                .returned_callable_bindings(returned, bindings, state, application, environment)
                .ok()
                .map(|sources| sources.into_iter().collect());
        }
        let mut result = BTreeSet::new();
        for source in returned {
            let target = match source {
                CallableReturnSource::Projection(id) => crate::ParameterCallTarget::Projection(*id),
                CallableReturnSource::Parameter(symbol) => {
                    crate::ParameterCallTarget::Native(*symbol)
                }
                _ => return None,
            };
            let selected = match self
                .trace_argument(target, "returned payload", bindings, state)
                .ok()?
            {
                Some(TraceArgumentBinding::Expression(expr)) => {
                    self.projected_sources(expr, suffix, state)?
                }
                Some(TraceArgumentBinding::ProjectedExpression { expr, mut path }) => {
                    path.extend_from_slice(suffix);
                    self.projected_sources(expr, &path, state)?
                }
                Some(TraceArgumentBinding::Origins(origins)) => {
                    self.project_value_origins(&origins, suffix, state, application)?
                }
                Some(TraceArgumentBinding::ProjectedOrigins { sources, mut path }) => {
                    path.extend_from_slice(suffix);
                    self.project_value_origins(&sources, &path, state, application)?
                }
                Some(TraceArgumentBinding::AgentTools(_)) => return None,
                None => {
                    let roots = self
                        .returned_callable_bindings(
                            &BTreeSet::from([source.clone()]),
                            bindings,
                            state,
                            application,
                            environment,
                        )
                        .ok()?;
                    self.project_value_origins(
                        &roots.into_iter().collect(),
                        suffix,
                        state,
                        application,
                    )?
                }
            };
            result.extend(selected);
        }
        Some(result)
    }

    pub(super) fn project_value_origins(
        &self,
        origins: &BTreeSet<LatentSource>,
        path: &[Step],
        state: &EffectState,
        application: HirExprId,
    ) -> Option<BTreeSet<LatentSource>> {
        if path.is_empty() {
            return Some(origins.clone());
        }
        if let Some(sources) = self.project_parameter_origins(origins.iter().copied(), path) {
            return Some(sources);
        }
        let [Step::CallResult, rest @ ..] = path else {
            return None;
        };
        self.invoked_value_projection(
            &origins.iter().copied().collect::<Vec<_>>(),
            rest,
            state,
            application,
        )
    }

    pub(super) fn invoked_value_projection(
        &self,
        origins: &[LatentSource],
        path: &[Step],
        state: &EffectState,
        application: HirExprId,
    ) -> Option<BTreeSet<LatentSource>> {
        let mut result = BTreeSet::new();
        let mut seen = BTreeSet::new();
        for source in origins {
            if !seen.insert(*source) {
                continue;
            }
            if matches!(
                source,
                LatentSource::Parameter { .. } | LatentSource::Projection(_)
            ) {
                let mut selected = vec![Step::CallResult];
                selected.extend_from_slice(path);
                result.extend(self.project_parameter_origins([*source], &selected)?);
                continue;
            }
            if let LatentSource::Metadata(source) = source {
                result.extend(self.metadata_return_projection(
                    *source,
                    path,
                    state,
                    application,
                )?);
                continue;
            }
            let (unit, environment) = match source {
                LatentSource::Unit(unit) => (*unit, None),
                LatentSource::Captured { unit, environment } => (*unit, Some(*environment)),
                LatentSource::Reference {
                    unit, environment, ..
                } => (*unit, *environment),
                _ => return None,
            };
            if !self.invocation_params(unit, environment).ok()??.is_empty() {
                return None;
            }
            if !matches!(unit, EffectUnit::Item(_) | EffectUnit::AnonymousFlow { .. }) {
                return None;
            }
            let summary = self
                .latent_source_summary(*source, self.hir.exprs[application].span(&self.hir.blocks))
                .ok()?;
            let exits = summary.exits.as_ref()?;
            let bindings = environment
                .map(|id| self.native_environment_bindings(id))
                .transpose()
                .ok()?
                .unwrap_or_default();
            if path.is_empty() {
                result.extend(self.projected_return_bindings(
                    &exits.callable_return_sources()?,
                    &[],
                    &bindings,
                    state,
                    application,
                    environment,
                )?);
                continue;
            }
            let selected = exits
                .value_projections
                .iter()
                .filter(|(prefix, _)| path.starts_with(prefix))
                .max_by_key(|(prefix, _)| prefix.len());
            let (returned, suffix) = match selected {
                Some((prefix, ProjectedReturnSources::Known(returned))) => {
                    (returned.clone(), &path[prefix.len()..])
                }
                Some((_, ProjectedReturnSources::Unavailable)) => return None,
                None => (exits.callable_return_sources()?, path),
            };
            result.extend(self.projected_return_bindings(
                &returned,
                suffix,
                &bindings,
                state,
                application,
                environment,
            )?);
        }
        Some(result)
    }
}
