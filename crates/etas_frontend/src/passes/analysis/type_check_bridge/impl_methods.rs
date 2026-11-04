use crate::code::descriptor::ProjectionError;
use etas_types::lower::external::{package_effect_row_input, package_type_input};

#[cfg(test)]
mod tests;

pub(super) fn signature(
    method: &etas_package_metadata::PublishedMethod,
) -> Result<etas_types::ExternalMethodInput, ProjectionError> {
    let invalid = |error: etas_types::lower::external::ExternalMetadataLoweringError| {
        ProjectionError::Invalid(error.message)
    };
    let root = method
        .root()
        .map_err(|error| ProjectionError::Invalid(error.to_string()))?;
    let parameters = |params: &[etas_package_metadata::GenericParam], scope| {
        params
            .iter()
            .enumerate()
            .map(|(position, param)| {
                Ok(etas_types::ExternalCallableGenericParamInput {
                    name: root
                        .generic_bindings
                        .iter()
                        .find(|binding| {
                            binding.scope == scope && binding.position as usize == position
                        })
                        .map_or_else(|| param.name.clone(), |binding| binding.label.clone()),
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
            .collect::<Result<_, ProjectionError>>()
    };
    Ok(etas_types::ExternalMethodInput {
        identity: method.identity.clone(),
        self_type: package_type_input(&method.self_type).map_err(invalid)?,
        receiver: match method.receiver {
            etas_package_metadata::MethodReceiver::Value { slot } => {
                etas_types::method::MethodReceiver::Value { slot }
            }
            etas_package_metadata::MethodReceiver::Associated => {
                etas_types::method::MethodReceiver::Associated
            }
            etas_package_metadata::MethodReceiver::Component => {
                etas_types::method::MethodReceiver::Component
            }
        },
        owner_params: parameters(
            &method.owner_params,
            etas_package_metadata::CallableValueScope::Root,
        )?,
        generic_params: parameters(
            &root.generic_params,
            etas_package_metadata::CallableValueScope::Value(0),
        )?,
        parameter_names: root.param_names.iter().cloned().map(Some).collect(),
        params: root
            .params
            .iter()
            .map(|ty| package_type_input(ty).map_err(invalid))
            .collect::<Result<_, _>>()?,
        output: package_type_input(&root.output).map_err(invalid)?,
        effects: package_effect_row_input(&root.public_effects).map_err(invalid)?,
    })
}
