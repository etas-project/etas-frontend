use std::collections::BTreeSet;

use etas_hir_analysis::interprocedural::CallSite;

use super::{
    engine::EffectSemantics,
    specialize::{EffectSpecializationError, TraceArgumentBinding},
    state::EffectState,
};
use crate::{EffectUnit, ParameterCallTarget, facts::CallableEnvironmentId};

impl EffectSemantics<'_> {
    pub(super) fn invoked_parameter_projections(
        summary: &crate::EffectSummary,
    ) -> BTreeSet<etas_code::CodeFingerprint> {
        summary
            .action_trace
            .parameter_calls()
            .into_iter()
            .filter_map(|call| match call.target {
                ParameterCallTarget::Projection(id) => Some(id),
                _ => None,
            })
            .collect()
    }

    /// Retain the checked values crossing this invocation boundary. Missing
    /// callback evidence stays unavailable; an unused parameter need not be
    /// callable just to record the arguments of an otherwise valid call.
    pub(super) fn record_source_call_arguments(
        &mut self,
        unit: EffectUnit,
        site: &CallSite<EffectUnit>,
        state: &EffectState,
        environment: Option<CallableEnvironmentId>,
        invoked_projections: &BTreeSet<etas_code::CodeFingerprint>,
    ) -> Result<(), EffectSpecializationError> {
        let Some(parameters) = self.invocation_params(unit, environment)? else {
            return Ok(());
        };
        let targets = parameters
            .into_iter()
            .map(ParameterCallTarget::Native)
            .collect::<Vec<_>>();
        let arguments = self.call_trace_bindings(site, &targets, state)?;
        for (target, argument) in &arguments {
            let ParameterCallTarget::Native(symbol) = *target else {
                unreachable!("native parameter interface");
            };
            if !self
                .symbol_value_type(symbol)
                .is_some_and(|ty| self.type_contains_flow(ty) || self.type_has_parameters(ty))
            {
                continue;
            }
            let sources = match argument {
                TraceArgumentBinding::Origins(sources) => sources.clone(),
                TraceArgumentBinding::Expression(expr) => self
                    .latent_sources_for_expr(state, *expr)
                    .map(|sources| sources.into_iter().collect())
                    .unwrap_or_else(|| {
                        if self.expr_type_contains_flow(*expr)
                            || self.expr_type_has_parameters(*expr)
                        {
                            BTreeSet::from([super::state::LatentSource::Unavailable {
                                expr: *expr,
                            }])
                        } else {
                            BTreeSet::new()
                        }
                    }),
                binding @ (TraceArgumentBinding::ProjectedExpression { .. }
                | TraceArgumentBinding::ProjectedOrigins { .. }
                | TraceArgumentBinding::InputData { .. }) => {
                    self.trace_argument_origins(binding, state)?
                }
                TraceArgumentBinding::AgentTools(_) => continue,
            };
            self.inputs
                .source_call_arguments
                .entry(site.call)
                .or_default()
                .entry(*target)
                .or_default()
                .extend(sources);
        }
        let Some(owner) = unit.owner() else {
            return Ok(());
        };
        // Rebinding every previously discovered path feeds projections from
        // recursive calls back into themselves. Only actual invocations need
        // a projected argument at this execution boundary.
        for id in invoked_projections {
            if !self
                .parameter_projection(*id)
                .is_some_and(|projection| projection.owner == owner)
            {
                continue;
            }
            let target = ParameterCallTarget::Projection(*id);
            let sources =
                match self.trace_argument(target, "invoked argument", &arguments, state)? {
                    Some(binding) => self.trace_argument_origins_at(&binding, state, site.call)?,
                    None => [super::state::LatentSource::Projection(*id)].into(),
                };
            self.inputs
                .source_call_arguments
                .entry(site.call)
                .or_default()
                .entry(ParameterCallTarget::Projection(*id))
                .or_default()
                .extend(sources);
        }
        Ok(())
    }
}
