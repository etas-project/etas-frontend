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
    let representation = None;
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

pub fn collect_members(ctx: &mut TypePipelineContext<'_>, state: &mut SignaturePipelineState) {
    use crate::{CallableSignature, EnumLayoutFact, EnumVariantLayoutFact};
    use etas_core::{Diagnostic, TypeDiagnosticCode};
    let items = ctx
        .hir
        .items
        .iter()
        .map(|(_, item)| item.clone())
        .collect::<Vec<_>>();
    for item in items {
        match item {
            HirItem::Type(decl) => {
                let representation = match decl.body {
                    HirTypeDeclBody::Bodyless => None,
                    HirTypeDeclBody::Representation(ty) => lower_type_ref(ctx, ty),
                };
                if let Some(SymbolTypeFact::NominalType {
                    constructor,
                    representation: slot,
                    ..
                }) = state.symbol_types.get_mut(&decl.symbol)
                {
                    ctx.interner
                        .define_nominal(crate::TypeId(constructor.0), representation);
                    *slot = representation;
                }
            }
            HirItem::Enum(decl) => {
                let SymbolTypeFact::Type { constructor } = state.symbol_types[&decl.symbol] else {
                    unreachable!("enum identity was predeclared")
                };
                let base = crate::TypeId(constructor.0);
                let generic_params =
                    super::callables::callable_generic_params(ctx, state, &decl.type_params);
                let output = if generic_params.is_empty() {
                    base
                } else {
                    ctx.interner.intern(Type::Applied {
                        constructor,
                        args: generic_params.iter().map(|param| param.subject).collect(),
                    })
                };
                let mut variants = Vec::new();
                let mut seen = std::collections::HashSet::new();
                for variant in decl.variants {
                    let name = ctx.hir.symbols.get(variant.symbol).unwrap().name.clone();
                    if !seen.insert(name.clone()) {
                        ctx.diagnostics.push(Diagnostic::type_check(
                            TypeDiagnosticCode::DuplicateField,
                            variant.span,
                            "duplicate enum variant",
                        ));
                    }
                    if let Some(names) = &variant.field_names {
                        let mut fields = std::collections::HashSet::new();
                        for name in names {
                            if !fields.insert(name) {
                                ctx.diagnostics.push(Diagnostic::type_check(
                                    TypeDiagnosticCode::DuplicateField,
                                    variant.span,
                                    "duplicate enum payload field",
                                ));
                            }
                        }
                    }
                    let fields = variant
                        .fields
                        .iter()
                        .filter_map(|ty| lower_type_ref(ctx, *ty))
                        .collect::<Vec<_>>();
                    state.symbol_types.insert(
                        variant.symbol,
                        SymbolTypeFact::Flow {
                            signature: CallableSignature {
                                generic_params: generic_params.clone(),
                                params: fields.clone(),
                                output,
                                effects: None,
                                requested_actions: None,
                            },
                        },
                    );
                    variants.push(EnumVariantLayoutFact {
                        name,
                        fields,
                        field_names: variant.field_names,
                    });
                }
                state.enum_layouts.insert(
                    base,
                    EnumLayoutFact {
                        type_params: generic_params.into_iter().map(|param| param.name).collect(),
                        variants,
                    },
                );
            }
            _ => {}
        }
    }
}
