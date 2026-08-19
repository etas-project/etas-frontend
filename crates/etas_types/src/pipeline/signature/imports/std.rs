use crate::{
    StdSignatureInput, StdSymbolBindingInput, SymbolTypeFact,
    lower::std::{lower_std_spec_impls, lower_std_symbol, lower_std_type_symbol},
    pipeline::context::TypePipelineContext,
};

pub fn apply_hir_std_prelude(ctx: &mut TypePipelineContext<'_>) {
    let bindings = ctx
        .hir
        .symbols
        .iter()
        .filter_map(|symbol| {
            let etas_hir::SymbolDef::ImportAlias { path, origin } = &symbol.def else {
                return None;
            };
            (*origin == etas_hir::ImportAliasOrigin::StdPrelude
                || path.first().is_some_and(|segment| segment == "std"))
            .then(|| StdSymbolBindingInput {
                symbol: symbol.id,
                qualified_path: path.clone(),
            })
        })
        .collect::<Vec<_>>();
    apply_std_signature_input(
        ctx,
        &StdSignatureInput {
            symbol_bindings: bindings,
        },
    );
}

pub fn apply_std_signature_input(ctx: &mut TypePipelineContext<'_>, input: &StdSignatureInput) {
    let registry = ctx.std_registry.clone();
    record_known_std_types(ctx, &registry);
    ctx.signature_facts.std_spec_impls = lower_std_spec_impls(ctx, &registry);
    for binding in &input.symbol_bindings {
        let Some(symbol) = registry.lookup_qualified(&binding.qualified_path) else {
            continue;
        };
        let fact = lower_std_symbol(ctx, &registry, binding.symbol, symbol);
        if matches!(
            symbol.decl,
            etas_std::StdDecl::Type(etas_std::TypeDecl {
                kind: etas_std::TypeDeclKind::Spec,
                ..
            })
        ) {
            ctx.signature_facts
                .std_spec_aliases
                .insert(binding.symbol, symbol.qualified_path.clone());
        }
        if let SymbolTypeFact::EffectAction { signature } = &fact {
            ctx.signature_facts
                .action_signatures
                .insert(binding.symbol, signature.clone());
        }
        ctx.signature_facts
            .symbol_types
            .insert(binding.symbol, fact);
    }
}

fn record_known_std_types(ctx: &mut TypePipelineContext<'_>, registry: &etas_std::StdRegistry) {
    let Some(index_error) = registry
        .lookup_qualified(&["std", "runtime", "error", "IndexError"])
        .and_then(|symbol| lower_std_type_symbol(ctx, registry, symbol))
    else {
        return;
    };
    ctx.signature_facts.known_std_types.index_error = Some(index_error);
}
