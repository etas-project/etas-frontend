use super::*;
use crate::lower::external::{ExternalMetadataLoweringError, package_type_input};
use etas_package_metadata::{ConstructorContract, GenericParamKind};

pub(super) fn parameters(
    contract: &ConstructorContract,
) -> Result<Vec<crate::ExternalCallableGenericParamInput>, ExternalMetadataLoweringError> {
    contract
        .generic_params
        .iter()
        .map(|parameter| {
            Ok(crate::ExternalCallableGenericParamInput {
                name: parameter.name.clone(),
                kind: match parameter.kind {
                    GenericParamKind::Type => crate::ExternalCallableGenericParamKindInput::Type,
                    GenericParamKind::Effect => {
                        crate::ExternalCallableGenericParamKindInput::Effect
                    }
                },
                bounds: parameter
                    .bounds
                    .iter()
                    .map(|bound| {
                        Ok(crate::ExternalSpecBoundInput {
                            spec: bound.spec.clone(),
                            args: bound
                                .args
                                .iter()
                                .map(package_type_input)
                                .collect::<Result<_, _>>()?,
                        })
                    })
                    .collect::<Result<_, ExternalMetadataLoweringError>>()?,
            })
        })
        .collect()
}

pub(super) fn declare(ctx: &mut TypePipelineContext<'_>, input: &ExternalSignatureInput) {
    // Mutually referring state types must share their opaque nominal identity.
    for metadata in &input.metadata {
        let Some(span) = package_binding_span(input, metadata.package) else {
            continue;
        };
        for agent in &metadata.agents {
            let contract = metadata
                .constructor_contracts
                .iter()
                .find(|contract| contract.agent && contract.path == agent.path);
            let params = match contract.map(parameters).transpose() {
                Ok(Some(params)) => params,
                Ok(None) => agent.generic_params.clone(),
                Err(error) => {
                    incomplete_external_type_facts(ctx, span, error.message);
                    continue;
                }
            };
            lower_external_type_declaration(ctx, span, &agent.path, &params, None);
        }
    }
}

pub(super) fn state(
    ctx: &mut TypePipelineContext<'_>,
    span: etas_core::Span,
    contract: &ConstructorContract,
) -> Result<TypeId, ExternalMetadataLoweringError> {
    let fields = contract
        .fields
        .iter()
        .map(|field| {
            let input = package_type_input(&field.ty)?;
            Ok(crate::FieldType {
                name: field.name.clone(),
                ty: lower_external_type(ctx, span, &input),
            })
        })
        .collect::<Result<_, ExternalMetadataLoweringError>>()?;
    Ok(ctx
        .interner
        .intern(Type::Record(crate::RecordType { fields })))
}
