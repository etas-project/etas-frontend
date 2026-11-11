use super::*;

impl ProjectMetadataProjection<'_> {
    pub(super) fn default_behavior_metadata(
        &self,
        summary: &etas_effects::EffectSummary,
        type_names: &BTreeSet<String>,
        effect_names: &BTreeSet<String>,
        labels: &BTreeMap<etas_effects::ParameterCallTarget, String>,
    ) -> Result<Option<etas_package_metadata::DefaultBehavior>, PackageMetadataError> {
        let Some(behavior) = &summary.default_behavior else {
            return Ok(None);
        };
        Ok(Some(etas_package_metadata::DefaultBehavior {
            direct: self.effect_footprint_metadata(
                &behavior.direct,
                type_names,
                effect_names,
                labels,
            )?,
            guarded: behavior
                .guarded
                .iter()
                .map(|(guards, footprint)| {
                    Ok(etas_package_metadata::GuardedEffectFootprint {
                        guards: guards
                            .iter()
                            .map(|guard| self.summary_effect_ref_metadata(guard, type_names))
                            .collect::<Result<_, _>>()?,
                        footprint: self.effect_footprint_metadata(
                            footprint,
                            type_names,
                            effect_names,
                            labels,
                        )?,
                    })
                })
                .collect::<Result<_, PackageMetadataError>>()?,
        }))
    }

    fn effect_footprint_metadata(
        &self,
        footprint: &etas_effects::EffectFootprint,
        type_names: &BTreeSet<String>,
        effect_names: &BTreeSet<String>,
        labels: &BTreeMap<etas_effects::ParameterCallTarget, String>,
    ) -> Result<etas_package_metadata::EffectFootprint, PackageMetadataError> {
        let summary = footprint.clone().into_summary();
        Ok(etas_package_metadata::EffectFootprint {
            escaping_effects: self.effect_row_metadata(
                &footprint.escaping_effects,
                type_names,
                effect_names,
            )?,
            requested_actions: self.effect_row_metadata(
                &footprint.requested_actions,
                type_names,
                effect_names,
            )?,
            default_actions: self.effect_row_metadata(
                &footprint.default_actions,
                type_names,
                effect_names,
            )?,
            handled_actions: self.effect_row_metadata(
                &footprint.handled_actions,
                type_names,
                effect_names,
            )?,
            handler_requests: self.request_template_metadata(
                &footprint.handler_requests,
                type_names,
                labels,
            )?,
            residual_checks: footprint.residual_checks.iter().cloned().collect(),
            behavior: callable_returns::behavior(&summary).ok_or_else(|| {
                invalid("default behavior footprint has no complete checked behavior")
            })?,
            requirements: self.summary_requirements(&summary)?,
        })
    }
}
