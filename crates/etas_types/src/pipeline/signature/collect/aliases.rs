use etas_hir::HirItem;

use crate::{
    SymbolTypeFact,
    lower::type_ref::lower_type_ref,
    pipeline::{context::TypePipelineContext, signature::state::SignaturePipelineState},
};

pub fn collect_aliases(ctx: &mut TypePipelineContext<'_>, state: &mut SignaturePipelineState) {
    let items = ctx
        .hir
        .items
        .iter()
        .map(|(id, item)| (id, item.clone()))
        .collect::<Vec<_>>();
    for (_, item) in items {
        let HirItem::TypeAlias(decl) = item else {
            continue;
        };
        let Some(target) = lower_type_ref(ctx, decl.target) else {
            continue;
        };
        let params = decl
            .type_params
            .iter()
            .filter_map(|symbol| {
                ctx.hir
                    .symbols
                    .get(*symbol)
                    .map(|symbol| symbol.name.clone())
            })
            .collect();
        let fact = SymbolTypeFact::TypeAlias { target, params };
        state.aliases.insert(decl.symbol, target);
        state.symbol_types.insert(decl.symbol, fact.clone());
        ctx.signature_facts.symbol_types.insert(decl.symbol, fact);
    }
}
