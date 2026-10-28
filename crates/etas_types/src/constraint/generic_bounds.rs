use etas_core::{Span, TypeDiagnosticCode};

use crate::{
    CallableGenericParam, CallableGenericParamKind, SpecObligation, TypeId, TypeInterner,
    ValidationRequest,
};

pub(crate) fn instantiate(
    interner: &mut TypeInterner,
    parameters: &[CallableGenericParam],
    arguments: &[TypeId],
    span: Span,
) -> Result<Vec<SpecObligation>, ValidationRequest> {
    if parameters.len() != arguments.len()
        || parameters
            .iter()
            .any(|param| param.kind != CallableGenericParamKind::Type)
    {
        return Err(ValidationRequest::Diagnostic {
            code: TypeDiagnosticCode::ArityMismatch,
            span,
            message: "type arguments do not match the declaration's type parameters".into(),
        });
    }
    let substitutions = parameters
        .iter()
        .zip(arguments)
        .map(|(param, ty)| (param.name.clone(), *ty))
        .collect();
    let mut obligations = Vec::new();
    for (parameter, ty) in parameters.iter().zip(arguments) {
        for bound in &parameter.bounds {
            let args = bound
                .args
                .iter()
                .map(|arg| crate::substitute_named_params(interner, *arg, &substitutions))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| ValidationRequest::Diagnostic {
                    code: TypeDiagnosticCode::IncompleteTypeFacts,
                    span,
                    message: format!("generic spec bound substitution failed: {error}"),
                })?;
            obligations.push(SpecObligation {
                ty: *ty,
                spec: bound.spec.clone(),
                args,
                span,
            });
        }
    }
    Ok(obligations)
}
