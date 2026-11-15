use super::*;

pub(super) fn lower(
    ctx: &mut TypePipelineContext<'_>,
    span: etas_core::Span,
    package: ExternalPackageKey,
    item: &crate::ExternalNamedSignatureInput,
) -> Option<Vec<(Vec<String>, SymbolTypeFact)>> {
    let Some(declaration @ crate::ExternalTypeInput::Enum { variants, .. }) = &item.ty else {
        incomplete_external_type_facts(ctx, span, "external enum has no checked layout".into());
        return None;
    };
    let ty = lower_external_type(ctx, span, declaration);
    let parameters = lower_external_callable_generic_params(
        ctx,
        span,
        package,
        &item.path,
        &item.generic_params,
    )?;
    let result_role = item.path == ["std", "result", "Result"]
        && ctx
            .std_registry
            .lookup_qualified(&item.path)
            .is_some_and(|symbol| ctx.std_registry.declaration_key(symbol.id).is_some());
    let option_role = item.path == ["std", "option", "Option"]
        && ctx
            .std_registry
            .lookup_qualified(&item.path)
            .is_some_and(|symbol| ctx.std_registry.declaration_key(symbol.id).is_some());
    let output = if result_role {
        ctx.interner.intern(Type::Result {
            ok: parameters[0].subject,
            err: parameters[1].subject,
        })
    } else if option_role {
        ctx.interner.intern(Type::Option(parameters[0].subject))
    } else if parameters.is_empty() {
        ty
    } else {
        ctx.interner.intern(Type::Applied {
            constructor: TypeConstructorId(ty.0),
            args: parameters.iter().map(|param| param.subject).collect(),
        })
    };
    let mut facts = Vec::new();
    let mut layouts = Vec::new();
    for variant in variants {
        let (fields, field_names) = match &variant.ty {
            crate::ExternalTypeInput::Tuple(fields) => (
                fields
                    .iter()
                    .map(|field| lower_external_type(ctx, span, field))
                    .collect::<Vec<_>>(),
                None,
            ),
            crate::ExternalTypeInput::Record { fields } => (
                fields
                    .iter()
                    .map(|field| lower_external_type(ctx, span, &field.ty))
                    .collect(),
                Some(fields.iter().map(|field| field.name.clone()).collect()),
            ),
            _ => {
                incomplete_external_type_facts(
                    ctx,
                    span,
                    "external enum has invalid payload form".into(),
                );
                return None;
            }
        };
        let mut path = item.path.clone();
        path.push(variant.name.clone());
        facts.push((
            path,
            SymbolTypeFact::Flow {
                signature: CallableSignature {
                    generic_params: parameters.clone(),
                    params: fields.clone(),
                    output,
                    effects: None,
                    requested_actions: None,
                },
            },
        ));
        layouts.push(crate::EnumVariantLayoutFact {
            name: variant.name.clone(),
            fields,
            field_names,
        });
    }
    ctx.signature_facts.enum_layouts.insert(
        ty,
        crate::EnumLayoutFact {
            type_params: item
                .generic_params
                .iter()
                .map(|param| param.name.clone())
                .collect(),
            variants: layouts,
        },
    );
    ctx.signature_facts.type_generics.insert(ty, parameters);
    if result_role {
        ctx.signature_facts.known_std_types.result_enum = Some(ty);
    }
    if option_role {
        ctx.signature_facts.known_std_types.option_enum = Some(ty);
    }
    facts.push((
        item.path.clone(),
        SymbolTypeFact::Type {
            constructor: TypeConstructorId(ty.0),
        },
    ));
    Some(facts)
}
