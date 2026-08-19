use etas_hir::{HirImplItem, HirImplTarget, HirItem, HirSpecItem, ResolveResult};

use crate::{
    CallableSignature, PrimitiveType, SpecImplFact, SpecImplMethodFact, SpecKind, SpecMethodFact,
    SpecMethodIdentity, SpecSignature, SpecSuperBoundFact, SymbolTypeFact, TypeParamBoundFact,
    TypeSpecSatisfactionFact,
    lower::{effect_row::lower_effect_row, type_ref::lower_type_ref},
    pipeline::{context::TypePipelineContext, signature::state::SignaturePipelineState},
};

pub fn collect_specs(ctx: &mut TypePipelineContext<'_>, state: &mut SignaturePipelineState) {
    let items = ctx
        .hir
        .items
        .iter()
        .map(|(_, item)| item.clone())
        .collect::<Vec<_>>();
    for (impl_group, item) in items.into_iter().enumerate() {
        match item {
            HirItem::Spec(decl) => {
                let super_specs = decl
                    .bounds
                    .iter()
                    .filter_map(|bound| match bound.path.resolution {
                        ResolveResult::Resolved(super_spec_symbol) => {
                            let super_spec_symbol = ctx
                                .symbols
                                .canonical_symbol(ctx.hir, super_spec_symbol)
                                .unwrap_or(super_spec_symbol);
                            Some(SpecSuperBoundFact {
                                spec_symbol: decl.symbol,
                                super_spec_symbol,
                                args: bound
                                    .args
                                    .iter()
                                    .filter_map(|arg| lower_type_ref(ctx, *arg))
                                    .collect(),
                            })
                        }
                        _ => None,
                    })
                    .collect();
                state.spec_signatures.insert(
                    decl.symbol,
                    SpecSignature {
                        symbol: decl.symbol,
                        name: symbol_name(ctx, decl.symbol),
                        kind: match decl.kind {
                            etas_hir::HirSpecKind::TypeSpec => SpecKind::TypeSpec,
                            etas_hir::HirSpecKind::CallableSpec => SpecKind::CallableSpec,
                            etas_hir::HirSpecKind::TraceSpec => SpecKind::TraceSpec,
                        },
                        param_names: decl
                            .type_params
                            .iter()
                            .filter_map(|param| ctx.hir.symbols.get(*param))
                            .map(|symbol| symbol.name.clone())
                            .collect(),
                        params: decl.type_params,
                        callable: decl
                            .callable
                            .as_ref()
                            .map(|callable| callable_signature_from_spec_callable(ctx, callable)),
                        methods: decl
                            .items
                            .iter()
                            .filter_map(|item| match item {
                                HirSpecItem::FlowSignature(signature) => Some(SpecMethodFact {
                                    identity: SpecMethodIdentity::Source(signature.symbol),
                                    name: symbol_name(ctx, signature.symbol),
                                    signature: None,
                                }),
                                HirSpecItem::Error { .. } => None,
                            })
                            .collect(),
                        super_specs,
                    },
                );
                for item in decl.items {
                    if let HirSpecItem::FlowSignature(signature) = item {
                        let callable = callable_signature_from_spec_item(ctx, &signature);
                        state.symbol_types.insert(
                            signature.symbol,
                            SymbolTypeFact::Flow {
                                signature: callable,
                            },
                        );
                    }
                }
                state.symbol_types.insert(
                    decl.symbol,
                    SymbolTypeFact::Spec {
                        symbol: decl.symbol,
                    },
                );
            }
            HirItem::Impl(decl) => {
                let impl_methods = decl
                    .items
                    .iter()
                    .filter_map(|item| match item {
                        HirImplItem::Flow(flow) => Some(SpecImplMethodFact {
                            symbol: flow.symbol,
                            name: symbol_name(ctx, flow.symbol),
                        }),
                        HirImplItem::Action(_) | HirImplItem::Error { .. } => None,
                    })
                    .collect::<Vec<_>>();
                for item in &decl.items {
                    if let HirImplItem::Flow(flow) = item {
                        let callable = callable_signature_from_flow(ctx, flow);
                        state.symbol_types.insert(
                            flow.symbol,
                            SymbolTypeFact::Flow {
                                signature: callable,
                            },
                        );
                    }
                }
                if let HirImplTarget::SpecSatisfaction { specs, self_type } = decl.target {
                    let Some(self_type) = lower_type_ref(ctx, self_type) else {
                        continue;
                    };
                    for spec_ref in specs {
                        if let ResolveResult::Resolved(spec_symbol) = spec_ref.spec_path.resolution
                        {
                            let spec_symbol = ctx
                                .symbols
                                .canonical_symbol(ctx.hir, spec_symbol)
                                .unwrap_or(spec_symbol);
                            let methods = if state
                                .spec_signatures
                                .get(&spec_symbol)
                                .is_some_and(|signature| !signature.methods.is_empty())
                            {
                                impl_methods.clone()
                            } else {
                                Vec::new()
                            };
                            state.spec_impls.push(SpecImplFact {
                                self_type,
                                spec_symbol,
                                args: spec_ref
                                    .spec_args
                                    .iter()
                                    .filter_map(|arg| lower_type_ref(ctx, *arg))
                                    .collect(),
                                methods,
                                impl_group: impl_group as u32,
                                span: spec_ref.span,
                            });
                            if spec_signature(ctx, state, spec_symbol).is_some_and(|signature| {
                                matches!(signature.kind, SpecKind::TypeSpec)
                            }) {
                                state
                                    .type_spec_satisfactions
                                    .push(TypeSpecSatisfactionFact {
                                        self_type,
                                        spec_symbol,
                                        args: spec_ref
                                            .spec_args
                                            .iter()
                                            .filter_map(|arg| lower_type_ref(ctx, *arg))
                                            .collect(),
                                        impl_group: impl_group as u32,
                                        span: spec_ref.span,
                                    });
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn spec_signature<'a>(
    ctx: &'a TypePipelineContext<'_>,
    state: &'a SignaturePipelineState,
    symbol: etas_hir::SymbolId,
) -> Option<&'a SpecSignature> {
    state
        .spec_signatures
        .get(&symbol)
        .or_else(|| ctx.signature_facts.spec_signatures.get(&symbol))
}

fn symbol_name(ctx: &TypePipelineContext<'_>, symbol: etas_hir::SymbolId) -> String {
    let name = ctx
        .hir
        .symbols
        .get(symbol)
        .map(|symbol| symbol.name.clone())
        .unwrap_or_else(|| format!("symbol{}", symbol.0));
    name.rsplit('.').next().unwrap_or(&name).to_owned()
}

fn callable_signature_from_spec_item(
    ctx: &mut TypePipelineContext<'_>,
    signature: &etas_hir::HirFlowSignature,
) -> CallableSignature {
    CallableSignature {
        generic_params: Vec::new(),
        params: lower_param_types(ctx, &signature.params),
        output: signature
            .return_type
            .and_then(|ty| lower_type_ref(ctx, ty))
            .unwrap_or_else(|| ctx.interner.primitive(PrimitiveType::Unit)),
        effects: signature
            .effects
            .as_ref()
            .map(|row| lower_effect_row(ctx, row)),
        requested_actions: None,
    }
}

fn callable_signature_from_spec_callable(
    ctx: &mut TypePipelineContext<'_>,
    signature: &etas_hir::HirSpecCallableSignature,
) -> CallableSignature {
    let input = lower_type_ref(ctx, signature.input)
        .map(|ty| match ctx.interner.store().get(ty) {
            Some(crate::Type::Tuple(elems)) => elems.clone(),
            Some(crate::Type::Primitive(PrimitiveType::Unit)) => Vec::new(),
            _ => vec![ty],
        })
        .unwrap_or_default();
    CallableSignature {
        generic_params: Vec::new(),
        params: input,
        output: lower_type_ref(ctx, signature.output)
            .unwrap_or_else(|| ctx.interner.primitive(PrimitiveType::Never)),
        effects: signature
            .effects
            .as_ref()
            .map(|row| lower_effect_row(ctx, row)),
        requested_actions: None,
    }
}

fn callable_signature_from_flow(
    ctx: &mut TypePipelineContext<'_>,
    flow: &etas_hir::HirFlowDecl,
) -> CallableSignature {
    CallableSignature {
        generic_params: Vec::new(),
        params: lower_param_types(ctx, &flow.params),
        output: flow
            .return_type
            .and_then(|ty| lower_type_ref(ctx, ty))
            .unwrap_or_else(|| ctx.interner.primitive(PrimitiveType::Unit)),
        effects: flow.effects.as_ref().map(|row| lower_effect_row(ctx, row)),
        requested_actions: None,
    }
}

fn lower_param_types(
    ctx: &mut TypePipelineContext<'_>,
    params: &[etas_hir::SymbolId],
) -> Vec<crate::TypeId> {
    params
        .iter()
        .map(|symbol| {
            let ty = ctx
                .hir
                .symbols
                .get(*symbol)
                .and_then(|symbol| symbol.declared_type)
                .and_then(|ty| lower_type_ref(ctx, ty))
                .unwrap_or_else(|| ctx.interner.primitive(PrimitiveType::Never));
            ctx.signature_facts
                .symbol_types
                .insert(*symbol, SymbolTypeFact::Param { ty });
            ty
        })
        .collect()
}

pub fn record_type_param_bounds(
    ctx: &mut TypePipelineContext<'_>,
    state: &mut SignaturePipelineState,
) {
    for symbol in ctx.hir.symbols.iter() {
        if let etas_hir::SymbolDef::TypeParam { bounds, .. } = &symbol.def {
            let facts = bounds
                .iter()
                .filter_map(|bound| match bound.path.resolution {
                    ResolveResult::Resolved(spec_symbol) => {
                        let spec_symbol = ctx
                            .symbols
                            .canonical_symbol(ctx.hir, spec_symbol)
                            .unwrap_or(spec_symbol);
                        Some(TypeParamBoundFact {
                            param: symbol.id,
                            param_name: symbol.name.clone(),
                            spec_symbol,
                            args: bound
                                .args
                                .iter()
                                .filter_map(|arg| lower_type_ref(ctx, *arg))
                                .collect(),
                        })
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            if !facts.is_empty() {
                state.type_param_bounds.insert(symbol.id, facts);
            }
        }
    }
}
