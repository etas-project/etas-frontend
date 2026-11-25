use super::*;

pub(super) fn prepare(
    ctx: &mut TypePipelineContext<'_>,
    metadata: &crate::ExternalPublicMetadataInput,
    span: etas_core::Span,
) {
    for implementation in &metadata.spec_impls {
        let Some(owner) = &implementation.owner else {
            continue;
        };
        if let Err(reason) = crate::lower::external::reserve_external_generic_parameters(
            ctx,
            &implementation.generic_params,
            Some(&etas_code::descriptor::GenericOwner::Implementation(
                owner.clone(),
            )),
        ) {
            incomplete_external_type_facts(ctx, span, reason.into());
        }
    }
    for method in &metadata.methods {
        let owner = etas_code::descriptor::GenericOwner::Implementation(
            method.identity.implementation().clone(),
        );
        if let Err(reason) = crate::lower::external::reserve_external_generic_parameters(
            ctx,
            &method.owner_params,
            Some(&owner),
        ) {
            ctx.diagnostics.push(etas_core::Diagnostic::type_check(
                etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span,
                reason,
            ));
        }
    }
}

pub(super) fn apply(
    ctx: &mut TypePipelineContext<'_>,
    metadata: &crate::ExternalPublicMetadataInput,
    span: etas_core::Span,
) {
    for method in &metadata.methods {
        if method.identity.implementation().subject().kind() == etas_code::DefinitionKind::Agent {
            ctx.interner
                .intern(Type::AgentContext(crate::AgentContextRef::Portable(
                    Box::new(method.identity.clone()),
                )));
        }
        let label = format!("{:?}", method.identity);
        let Some(owner_params) = crate::lower::external::lower_external_scoped_generic_params(
            ctx,
            span,
            metadata.package,
            &label,
            &method.owner_params,
            Some(etas_code::descriptor::GenericOwner::Implementation(
                method.identity.implementation().clone(),
            )),
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
            parameter_interface: Some(
                metadata
                    .method_parameter_interfaces
                    .get(&method.identity)
                    .cloned()
                    .unwrap_or_else(|| {
                        crate::call_interface::required_parameters(&method.parameter_names)
                    }),
            ),
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
            if let Some(parameters) = &mut signature.parameter_interface {
                parameters.insert(
                    0,
                    crate::ParameterInterface {
                        name: None,
                        default: None,
                    },
                );
            }
            parameter_names.insert(0, None);
        }
        ctx.signature_facts
            .external_methods
            .push(crate::facts::methods::ExternalMethodFact {
                span,
                identity: method.identity.clone(),
                internal: method.internal,
                self_type,
                receiver: method.receiver,
                owner_params,
                signature,
                parameter_names,
                implementations: Vec::new(),
            });
    }
}
