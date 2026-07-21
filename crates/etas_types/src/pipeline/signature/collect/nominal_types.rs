use etas_hir::{HirItem, HirItemId, HirTypeDeclBody};

use crate::{
    EnumTypeRef, NominalTypeRef, SymbolTypeFact, Type, TypeConstructorId,
    lower::type_ref::{canonical_source_type_name, lower_type_ref},
    pipeline::{context::TypePipelineContext, signature::state::SignaturePipelineState},
};

pub fn collect_nominal_types(
    ctx: &mut TypePipelineContext<'_>,
    state: &mut SignaturePipelineState,
) {
    let items = ctx
        .hir
        .items
        .iter()
        .map(|(id, item)| (id, item.clone()))
        .collect::<Vec<_>>();
    for (id, item) in items {
        match item {
            HirItem::Type(decl) => collect_type_decl(ctx, state, id, decl),
            HirItem::Enum(decl) => collect_enum_decl(ctx, state, decl),
            _ => {}
        }
    }
}

fn collect_type_decl(
    ctx: &mut TypePipelineContext<'_>,
    state: &mut SignaturePipelineState,
    _id: HirItemId,
    decl: etas_hir::HirTypeDecl,
) {
    let name = canonical_source_type_name(ctx, decl.symbol)
        .unwrap_or_else(|| format!("type{}", decl.symbol.0));
    let representation = match decl.body {
        HirTypeDeclBody::Bodyless => None,
        HirTypeDeclBody::Representation(ty) => lower_type_ref(ctx, ty),
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
        .collect::<Vec<_>>();
    let ty = ctx.interner.intern(Type::Nominal(NominalTypeRef {
        name,
        params: params.clone(),
        representation,
    }));
    state.symbol_types.insert(
        decl.symbol,
        SymbolTypeFact::NominalType {
            constructor: TypeConstructorId(ty.0),
            params,
            representation,
        },
    );
}

fn collect_enum_decl(
    ctx: &mut TypePipelineContext<'_>,
    state: &mut SignaturePipelineState,
    decl: etas_hir::HirEnumDecl,
) {
    let name = canonical_source_type_name(ctx, decl.symbol)
        .unwrap_or_else(|| format!("enum{}", decl.symbol.0));
    let ty = ctx.interner.intern(Type::Enum(EnumTypeRef { name }));
    state.symbol_types.insert(
        decl.symbol,
        SymbolTypeFact::Type {
            constructor: TypeConstructorId(ty.0),
        },
    );
}
