use super::*;

pub(super) struct ProjectedSummary {
    pub handler_requests: Option<etas_package_metadata::RequestInstantiationTemplate>,
    pub behavior: etas_package_metadata::CallableValueBehavior,
    pub public_effects: MetadataEffectRow,
    pub requested_actions: MetadataEffectRow,
    pub handled_requested_actions: MetadataEffectRow,
    pub default_handler_effects: etas_package_metadata::DefaultHandlerEffects,
    pub action_trace: MetadataActionTrace,
}

pub(super) fn project(
    projection: &ProjectMetadataProjection<'_>,
    summary: &EffectSummary,
    type_names: &BTreeSet<String>,
    effect_names: &BTreeSet<String>,
    labels: &BTreeMap<etas_effects::ParameterCallTarget, String>,
) -> Result<Option<ProjectedSummary>, PackageMetadataError> {
    if !summary.residual_checks.checks.is_empty()
        || summary.requirements.iter().next().is_some()
        || summary.trace_spec_obligations.iter().next().is_some()
    {
        return Ok(None);
    }
    let Some(behavior) = behavior(summary) else {
        return Ok(None);
    };
    Ok(Some(ProjectedSummary {
        handler_requests: projection.request_template_metadata(
            &summary.handler_requests,
            type_names,
            labels,
        )?,
        behavior,
        public_effects: projection.effect_row_metadata(
            &summary.escaping_effects,
            type_names,
            effect_names,
        )?,
        requested_actions: projection.effect_row_metadata(
            &summary.requested_actions,
            type_names,
            effect_names,
        )?,
        handled_requested_actions: projection.handled_requested_action_row(
            summary,
            type_names,
            effect_names,
        )?,
        default_handler_effects: etas_package_metadata::DefaultHandlerEffects {
            direct_effects: projection.effect_row_metadata(
                summary.direct_escaping_effects(),
                type_names,
                effect_names,
            )?,
            default_actions: projection.effect_row_metadata(
                &summary.default_actions,
                type_names,
                effect_names,
            )?,
            handlers: summary
                .default_handler_effects
                .iter()
                .flat_map(|effects| &effects.handlers)
                .map(|handler| {
                    Ok(etas_package_metadata::DefaultHandlerContribution {
                        action: projection
                            .summary_effect_ref_metadata(&handler.action, type_names)?,
                        produced: projection.effect_row_metadata(
                            &handler.produced,
                            type_names,
                            effect_names,
                        )?,
                    })
                })
                .collect::<Result<_, PackageMetadataError>>()?,
        },
        action_trace: projection.action_trace_metadata(
            &summary.action_trace,
            type_names,
            labels,
        )?,
    }))
}
