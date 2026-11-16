use super::{engine::EffectSemantics, metadata_callable::behavior};
use crate::{
    DefaultBehavior, EffectFootprint, EffectSet, ExternalDefaultBehaviorMetadata,
    ExternalEffectFootprintMetadata, ParameterCallTarget, RequestInstantiationSummary,
};
use etas_core::Span;

impl EffectSemantics<'_> {
    pub(super) fn external_default_behavior(
        &self,
        input: &ExternalDefaultBehaviorMetadata,
        parameters: &[(String, ParameterCallTarget)],
        generics: &[(String, etas_types::CallableGenericParamKind)],
        span: Span,
    ) -> Option<DefaultBehavior> {
        let direct = self.external_effect_footprint(&input.direct, parameters, generics, span)?;
        let mut guarded = std::collections::BTreeMap::new();
        for (guards, footprint) in &input.guarded {
            let guards = guards
                .iter()
                .map(|guard| {
                    let action = self.external_effect_from_metadata(guard)?;
                    action.is_action().then_some(action)
                })
                .collect::<Option<EffectSet>>()?;
            if guards.is_empty() {
                return None;
            }
            let footprint =
                self.external_effect_footprint(footprint, parameters, generics, span)?;
            if guarded.insert(guards, footprint).is_some() {
                return None;
            }
        }
        Some(DefaultBehavior { direct, guarded })
    }

    fn external_effect_footprint(
        &self,
        input: &ExternalEffectFootprintMetadata,
        parameters: &[(String, ParameterCallTarget)],
        generics: &[(String, etas_types::CallableGenericParamKind)],
        span: Span,
    ) -> Option<EffectFootprint> {
        let handler_requests = match &input.handler_requests {
            None => RequestInstantiationSummary::Unavailable,
            Some(requests) => RequestInstantiationSummary::Available {
                actions: requests
                    .actions
                    .iter()
                    .map(|request| {
                        Some(crate::ActionRequestInstance {
                            action: self.external_effect_from_metadata(&request.action)?,
                            type_bindings: request.type_bindings.clone(),
                            signature: request.signature.clone(),
                        })
                    })
                    .collect::<Option<_>>()?,
                parameters: requests
                    .parameters
                    .iter()
                    .map(|request| {
                        let target = self.external_request_target(
                            &request.parameter,
                            parameters,
                            generics,
                            span,
                        )?;
                        let parameter = match &request.parameter {
                            crate::ExternalCallableRequestTarget::Parameter(name) => name.clone(),
                            crate::ExternalCallableRequestTarget::SpecMethod(method) => {
                                method.method.clone()
                            }
                            crate::ExternalCallableRequestTarget::AgentTools(_) => {
                                "agent tools".into()
                            }
                        };
                        Some(crate::ParameterRequest {
                            target,
                            parameter,
                            span,
                            excluded: request
                                .excluded
                                .iter()
                                .map(|effect| self.external_effect_from_metadata(effect))
                                .collect::<Option<_>>()?,
                        })
                    })
                    .collect::<Option<_>>()?,
            },
        };
        let (requirements, trace_spec_obligations) =
            self.method_requirements(&input.requirements).ok()?;
        let (determinism, support) = behavior::lower(&input.behavior);
        let mut residual_checks = crate::ResidualCheckSet::default();
        for reference in &input.residual_checks {
            residual_checks.insert(reference.clone());
        }
        Some(EffectFootprint {
            handler_requests,
            escaping_effects: self.external_effect_row_from_metadata(&input.escaping_effects)?,
            requested_actions: self.external_effect_row_from_metadata(&input.requested_actions)?,
            default_actions: self.external_effect_row_from_metadata(&input.default_actions)?,
            handled_actions: self.external_effect_row_from_metadata(&input.handled_actions)?,
            residual_checks,
            trace_spec_obligations,
            requirements,
            determinism,
            independent_determinism: input
                .behavior
                .independent_determinism
                .map(behavior::lower_determinism),
            support,
        })
    }
}
