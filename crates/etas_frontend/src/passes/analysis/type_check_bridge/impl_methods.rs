use crate::code::descriptor::ProjectionError;
use etas_types::lower::external::{package_effect_row_input, package_type_input};

pub(super) fn signature(
    method: &etas_package_metadata::SpecImplMethod,
) -> Result<etas_types::ExternalSpecImplMethodInput, ProjectionError> {
    let invalid = |error: etas_types::lower::external::ExternalMetadataLoweringError| {
        ProjectionError::Invalid(error.message)
    };
    let root = method
        .root()
        .map_err(|error| ProjectionError::Invalid(error.to_string()))?;
    let generic_params = root
        .generic_params
        .iter()
        .map(|param| {
            Ok(etas_types::ExternalCallableGenericParamInput {
                name: param.name.clone(),
                kind: match param.kind {
                    etas_package_metadata::GenericParamKind::Type => {
                        etas_types::ExternalCallableGenericParamKindInput::Type
                    }
                    etas_package_metadata::GenericParamKind::Effect => {
                        etas_types::ExternalCallableGenericParamKindInput::Effect
                    }
                },
                bounds: param
                    .bounds
                    .iter()
                    .map(|bound| {
                        Ok(etas_types::ExternalSpecBoundInput {
                            spec: bound.spec.clone(),
                            args: bound
                                .args
                                .iter()
                                .map(|ty| package_type_input(ty).map_err(invalid))
                                .collect::<Result<_, _>>()?,
                        })
                    })
                    .collect::<Result<_, ProjectionError>>()?,
            })
        })
        .collect::<Result<_, ProjectionError>>()?;
    Ok(etas_types::ExternalSpecImplMethodInput {
        identity: method.identity.clone(),
        generic_params,
        params: root
            .params
            .iter()
            .map(|ty| package_type_input(ty).map_err(invalid))
            .collect::<Result<_, _>>()?,
        output: package_type_input(&root.output).map_err(invalid)?,
        effects: package_effect_row_input(&root.public_effects).map_err(invalid)?,
    })
}
