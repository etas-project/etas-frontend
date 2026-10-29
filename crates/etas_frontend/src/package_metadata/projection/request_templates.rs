use super::*;

impl ProjectMetadataProjection<'_> {
    pub(in crate::package_metadata) fn item_request_template(
        &self,
        item: HirItemId,
        summary: &etas_effects::EffectSummary,
    ) -> Result<Option<etas_package_metadata::RequestInstantiationTemplate>, PackageMetadataError>
    {
        let names = self
            .checked
            .types
            .item_signatures
            .get(&item)
            .map(callable_type_generic_names)
            .ok_or_else(|| PackageMetadataError::InvalidMetadataType {
                reason: "request contract callable has no checked signature".into(),
            })?;
        self.request_template_metadata(
            &summary.handler_requests,
            &names,
            &self.trace_parameter_labels(item)?,
        )
    }

    pub(super) fn request_template_metadata(
        &self,
        summary: &etas_effects::RequestInstantiationSummary,
        type_names: &BTreeSet<String>,
        labels: &BTreeMap<etas_effects::ParameterCallTarget, String>,
    ) -> Result<Option<etas_package_metadata::RequestInstantiationTemplate>, PackageMetadataError>
    {
        let etas_effects::RequestInstantiationSummary::Available {
            actions,
            parameters,
        } = summary
        else {
            return Ok(None);
        };
        Ok(Some(etas_package_metadata::RequestInstantiationTemplate {
            actions: actions
                .iter()
                .map(|instance| {
                    Ok(etas_package_metadata::ActionRequestTemplate {
                        action: self.summary_effect_ref_metadata(&instance.action, type_names)?,
                        type_bindings: instance
                            .type_bindings
                            .as_ref()
                            .map(|bindings| {
                                bindings
                                    .iter()
                                    .map(|(parameter, ty)| {
                                        let mut ty = self.metadata_type(*ty)?;
                                        normalize_generic_type(&mut ty, type_names);
                                        Ok(etas_package_metadata::ActionTypeBinding {
                                            parameter: parameter.clone(),
                                            ty,
                                        })
                                    })
                                    .collect::<Result<_, PackageMetadataError>>()
                            })
                            .transpose()?,
                    })
                })
                .collect::<Result<_, PackageMetadataError>>()?,
            parameters: parameters
                .iter()
                .map(|parameter| {
                    Ok(etas_package_metadata::ParameterRequestTemplate {
                        parameter: self.callable_request_target(parameter.target, labels)?,
                        excluded: parameter
                            .excluded
                            .iter()
                            .map(|effect| self.summary_effect_ref_metadata(effect, type_names))
                            .collect::<Result<_, _>>()?,
                    })
                })
                .collect::<Result<_, PackageMetadataError>>()?,
        }))
    }
}
