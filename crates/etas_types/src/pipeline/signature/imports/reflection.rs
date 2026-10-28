use crate::{SymbolTypeFact, Type, TypeId, pipeline::context::TypePipelineContext};

pub fn initialize(ctx: &mut TypePipelineContext<'_>, input: &crate::api::ReflectionSignatureInput) {
    ctx.signature_facts
        .reflection_declaration_identities
        .extend(input.declarations.iter().cloned());
    ctx.signature_facts
        .reflection_effect_targets
        .extend(input.effect_targets.iter().cloned());
}

pub fn apply(ctx: &mut TypePipelineContext<'_>, input: &crate::api::ReflectionSignatureInput) {
    let mut imported = std::collections::HashMap::<_, Vec<_>>::new();
    for (symbol, identity) in &ctx.signature_facts.external_definition_bindings {
        let ty = match ctx.signature_facts.symbol_types.get(symbol) {
            Some(SymbolTypeFact::Agent { signature }) => Some(signature.component),
            Some(
                SymbolTypeFact::NominalType { constructor, .. }
                | SymbolTypeFact::Type { constructor },
            ) => Some(TypeId(constructor.0)),
            _ => None,
        };
        if let Some(ty) = ty {
            imported.entry(identity.clone()).or_default().push(ty);
        }
    }
    for (path, identity) in &input.declarations {
        if !matches!(
            identity.kind(),
            etas_code::DefinitionKind::Type | etas_code::DefinitionKind::Agent
        ) {
            continue;
        }
        let mut types = Vec::new();
        if let Some(ty) = ctx.external_type_paths.get(path) {
            types.push(*ty);
        }
        types.extend(imported.get(identity).into_iter().flatten().copied());
        for ty in types {
            if matches!(
                ctx.interner.store().get(ty),
                Some(Type::Nominal(_) | Type::Enum(_))
            ) {
                ctx.signature_facts
                    .reflection_type_identities
                    .insert(ty, identity.clone());
            }
        }
    }
}
