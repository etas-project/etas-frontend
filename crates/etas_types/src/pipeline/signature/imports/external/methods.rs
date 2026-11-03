use super::*;

pub(super) fn apply(
    ctx: &mut TypePipelineContext<'_>,
    metadata: &crate::ExternalPublicMetadataInput,
    span: etas_core::Span,
) {
    for method in &metadata.methods {
        let label = format!("{:?}", method.identity);
        let Some(owner_params) = crate::lower::external::lower_external_generic_params(
            ctx,
            span,
            metadata.package,
            &label,
            &method.owner_params,
        ) else {
            continue;
        };
        let Some(generic_params) = crate::lower::external::lower_external_generic_params(
            ctx,
            span,
            metadata.package,
            &label,
            &method.generic_params,
        ) else {
            continue;
        };
        let mut signature = CallableSignature {
            generic_params,
            params: method
                .params
                .iter()
                .map(|ty| lower_external_type(ctx, span, ty))
                .collect(),
            output: lower_external_type(ctx, span, &method.output),
            effects: Some(lower_external_effect_row(ctx, span, &method.effects)),
            requested_actions: None,
        };
        let self_type = lower_external_type(ctx, span, &method.self_type);
        let mut parameter_names = method.parameter_names.clone();
        if method.receiver == crate::method::MethodReceiver::Component {
            signature.params.insert(0, self_type);
            parameter_names.insert(0, None);
        }
        ctx.signature_facts
            .external_methods
            .push(crate::facts::methods::ExternalMethodFact {
                span,
                identity: method.identity.clone(),
                self_type,
                receiver: method.receiver,
                owner_params,
                signature,
                parameter_names,
                spec_member: None,
                evidence: crate::method::MethodEvidence::Inherent,
            });
    }
}
