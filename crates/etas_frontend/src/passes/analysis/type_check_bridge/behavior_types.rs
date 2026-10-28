use super::*;

pub(super) fn collect(
    metadata: &ProjectExternalPublicMetadataInput,
) -> Result<Vec<etas_types::ExternalTypeInput>, crate::code::descriptor::ProjectionError> {
    let mut types = Vec::new();
    for summary in &metadata.effect_summaries {
        if let Some(requests) = &summary.handler_requests {
            for action in &requests.actions {
                for (_, ty) in action.type_bindings.iter().flatten() {
                    types.push(convert_external_type(ty));
                }
                for arg in &action.action.args {
                    if let ProjectExternalEffectArgInput::Type(ty) = arg {
                        types.push(convert_external_type(ty));
                    }
                }
            }
            for effect in requests
                .parameters
                .iter()
                .flat_map(|parameter| &parameter.excluded)
            {
                for arg in &effect.args {
                    if let ProjectExternalEffectArgInput::Type(ty) = arg {
                        types.push(convert_external_type(ty));
                    }
                }
            }
        }
        if let Some(graph) = &summary.returned_callables {
            for node in &graph.values {
                for ty in node.params.iter().chain(std::iter::once(&node.output)) {
                    add(&mut types, ty)?;
                }
                if let Some(requests) = &node.handler_requests {
                    for action in &requests.actions {
                        for binding in action.type_bindings.iter().flatten() {
                            add(&mut types, &binding.ty)?;
                        }
                        for arg in &action.action.args {
                            if let Some(ty) = &arg.ty {
                                add(&mut types, ty)?;
                            }
                        }
                    }
                    for effect in requests
                        .parameters
                        .iter()
                        .flat_map(|parameter| &parameter.excluded)
                    {
                        for arg in &effect.args {
                            if let Some(ty) = &arg.ty {
                                add(&mut types, ty)?;
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(types)
}

fn add(
    types: &mut Vec<etas_types::ExternalTypeInput>,
    ty: &etas_package_metadata::Type,
) -> Result<(), crate::code::descriptor::ProjectionError> {
    types.push(
        etas_types::lower::external::package_type_input(ty)
            .map_err(|error| crate::code::descriptor::ProjectionError::Invalid(error.message))?,
    );
    Ok(())
}
