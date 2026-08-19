use etas_hir::{HirItem, HirToolBody};

use crate::{
    CallableGenericParam, CallableSignature, CheckedSpecBound, CheckedSpecRef, ItemSignature,
    NamedTypeRef, PrimitiveType, ResourceHandleFact, SymbolTypeFact, ToolSignature,
    TopLevelLetSignature, Type, TypeId,
    lower::{effect_row::lower_effect_row, type_ref::lower_type_ref},
    pipeline::{
        context::TypePipelineContext,
        signature::{collect::resource_handles, state::SignaturePipelineState},
    },
};

pub fn collect_callables(ctx: &mut TypePipelineContext<'_>, state: &mut SignaturePipelineState) {
    let items = ctx
        .hir
        .items
        .iter()
        .map(|(id, item)| (id, item.clone()))
        .collect::<Vec<_>>();
    for (id, item) in items {
        match item {
            HirItem::Flow(flow) => {
                let signature = CallableSignature {
                    generic_params: callable_generic_params(ctx, state, &flow.type_params),
                    params: lower_params(ctx, &flow.params),
                    output: flow
                        .return_type
                        .and_then(|ty| lower_type_ref(ctx, ty))
                        .unwrap_or_else(|| ctx.interner.primitive(PrimitiveType::Unit)),
                    effects: flow.effects.as_ref().map(|row| lower_effect_row(ctx, row)),
                    requested_actions: None,
                };
                state
                    .item_signatures
                    .insert(id, ItemSignature::Flow(signature.clone()));
                state
                    .symbol_types
                    .insert(flow.symbol, SymbolTypeFact::Flow { signature });
            }
            HirItem::Agent(agent) => {
                let Some(output_type) = agent.output_type else {
                    continue;
                };
                let signature = CallableSignature {
                    generic_params: Vec::new(),
                    params: lower_params(ctx, &agent.params),
                    output: lower_type_ref(ctx, output_type)
                        .unwrap_or_else(|| ctx.interner.primitive(PrimitiveType::Never)),
                    effects: agent.effects.as_ref().map(|row| lower_effect_row(ctx, row)),
                    requested_actions: None,
                };
                state
                    .item_signatures
                    .insert(id, ItemSignature::Agent(signature.clone()));
                state
                    .symbol_types
                    .insert(agent.symbol, SymbolTypeFact::Agent { signature });
            }
            HirItem::Tool(tool) => {
                let signature = ToolSignature {
                    generic_params: callable_generic_params(ctx, state, &tool.type_params),
                    params: lower_params(ctx, &tool.params),
                    output: lower_type_ref(ctx, tool.return_type)
                        .unwrap_or_else(|| ctx.interner.primitive(PrimitiveType::Never)),
                    effects: tool.effects.as_ref().map(|row| lower_effect_row(ctx, row)),
                    requested_actions: None,
                };
                state
                    .item_signatures
                    .insert(id, ItemSignature::Tool(signature.clone()));
                state
                    .symbol_types
                    .insert(tool.symbol, SymbolTypeFact::Tool { signature });
                if matches!(tool.body, HirToolBody::Decl { .. }) {
                    continue;
                }
            }
            HirItem::TopLevelLet(top) => {
                if let Some((ty, resource_handle)) = top_level_let_signature_type(ctx, &top) {
                    state
                        .item_signatures
                        .insert(id, ItemSignature::TopLevelLet(TopLevelLetSignature { ty }));
                    state.symbol_types.insert(
                        top.symbol,
                        SymbolTypeFact::TopLevelLet {
                            ty,
                            classification: top.classification,
                        },
                    );
                    if let Some(resource_handle) = resource_handle {
                        state.resource_handles.insert(top.symbol, resource_handle);
                    }
                }
            }
            _ => {}
        }
    }
}

fn callable_generic_params(
    ctx: &mut TypePipelineContext<'_>,
    state: &SignaturePipelineState,
    params: &[etas_hir::SymbolId],
) -> Vec<CallableGenericParam> {
    params
        .iter()
        .filter_map(|param| {
            let name = ctx.hir.symbols.get(*param)?.name.clone();
            let bounds = state
                .type_param_bounds
                .get(param)
                .cloned()
                .unwrap_or_default();
            Some(CallableGenericParam {
                subject: ctx
                    .interner
                    .intern(Type::Named(NamedTypeRef { name: name.clone() })),
                name,
                bounds: bounds
                    .into_iter()
                    .map(|bound| CheckedSpecBound {
                        spec: CheckedSpecRef::Source(bound.spec_symbol),
                        args: bound.args,
                    })
                    .collect(),
            })
        })
        .collect()
}

fn top_level_let_signature_type(
    ctx: &mut TypePipelineContext<'_>,
    top: &etas_hir::HirTopLevelLetDecl,
) -> Option<(TypeId, Option<ResourceHandleFact>)> {
    let annotated = top.type_annotation.and_then(|ty| lower_type_ref(ctx, ty));
    let resource_handle = resource_handles::top_level_resource_handle_signature(ctx, top.value);
    if let Some(ty) = annotated {
        return Some((ty, resource_handle.map(|(_, fact)| fact)));
    }
    resource_handle.map(|(ty, fact)| (ty, Some(fact)))
}

fn lower_params(ctx: &mut TypePipelineContext<'_>, params: &[etas_hir::SymbolId]) -> Vec<TypeId> {
    params
        .iter()
        .map(|symbol| {
            let ty = ctx
                .hir
                .symbols
                .get(*symbol)
                .and_then(|symbol| symbol.declared_type);
            let lowered = ty
                .and_then(|ty| lower_type_ref(ctx, ty))
                .unwrap_or_else(|| match ctx.hir.symbols.get(*symbol) {
                    Some(symbol) => {
                        ctx.unknown_type(symbol.definition_span, "parameter has no type annotation")
                    }
                    None => ctx.interner.primitive(PrimitiveType::Never),
                });
            ctx.signature_facts
                .symbol_types
                .insert(*symbol, SymbolTypeFact::Param { ty: lowered });
            lowered
        })
        .collect()
}
