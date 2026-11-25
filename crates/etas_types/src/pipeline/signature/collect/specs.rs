use etas_hir::{HirImplItem, HirImplTarget, HirItem, HirSpecItem, ResolveResult};

use crate::{
    CallableSignature, PrimitiveType, SpecImplFact, SpecImplMethodFact, SpecKind, SpecMethodFact,
    SpecMethodIdentity, SpecSignature, SpecSuperBoundFact, SymbolTypeFact, TypeParamBoundFact,
    TypeSpecSatisfactionFact,
    lower::{effect_row::lower_effect_row, type_ref::lower_type_ref},
    pipeline::{context::TypePipelineContext, signature::state::SignaturePipelineState},
};

pub fn collect_specs(ctx: &mut TypePipelineContext<'_>, state: &mut SignaturePipelineState) {
    let mut items = ctx
        .hir
        .items
        .iter()
        .map(|(item, declaration)| (item.0, declaration.clone()))
        .collect::<Vec<_>>();
    // Required methods belong to the spec regardless of source file order.
    // Preserve each impl's original group while collecting all specs first.
    items.sort_by_key(|(_, item)| !matches!(item, HirItem::Spec(_)));
    for (impl_group, item) in items {
        match item {
            HirItem::Spec(decl) => {
                if ctx.hir.source_standard_types {
                    let symbol = ctx
                        .hir
                        .symbols
                        .get(decl.symbol)
                        .expect("source spec symbol");
                    if let Some(module) = &ctx.hir.modules_arena[symbol.defining_module].name {
                        let mut path = module
                            .segments
                            .iter()
                            .map(|part| part.name.clone())
                            .collect::<Vec<_>>();
                        path.push(symbol.name.clone());
                        // The checked std source owns its declarations. New specs
                        // must not require a second declaration in Rust's legacy registry.
                        if path.first().is_some_and(|part| part == "std") {
                            ctx.signature_facts
                                .std_spec_aliases
                                .insert(decl.symbol, path);
                        }
                    }
                }
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
                                super_spec: crate::CheckedSpecRef::Source(super_spec_symbol),
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
                                    parameter_names: signature
                                        .params
                                        .iter()
                                        .map(|param| symbol_name(ctx, *param))
                                        .collect(),
                                    updating_receiver: signature.receiver_mode
                                        == etas_hir::HirReceiverMode::Updating,
                                    receiver_param: signature.receiver_type_param.and_then(
                                        |receiver| {
                                            signature
                                                .type_params
                                                .iter()
                                                .position(|param| *param == receiver)
                                                .map(|index| index as u32)
                                        },
                                    ),
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
                        let callable = callable_signature_from_spec_item(ctx, state, &signature);
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
                        HirImplItem::Callable { item } => match ctx.hir.items.get(*item) {
                            Some(HirItem::Flow(flow)) => Some(SpecImplMethodFact {
                                symbol: flow.symbol,
                                name: symbol_name(ctx, flow.symbol),
                            }),
                            _ => None,
                        },
                        HirImplItem::Action(_) | HirImplItem::Error { .. } => None,
                    })
                    .collect::<Vec<_>>();
                for item in &decl.items {
                    if let HirImplItem::Flow(flow) = item {
                        let callable = callable_signature_from_flow(ctx, state, flow);
                        state.symbol_types.insert(
                            flow.symbol,
                            SymbolTypeFact::Flow {
                                signature: callable,
                            },
                        );
                    }
                }
                let owner = decl.target.clone();
                if let HirImplTarget::SpecSatisfaction { specs, self_type } = decl.target {
                    let Some(self_type) = lower_type_ref(ctx, self_type) else {
                        continue;
                    };
                    let mut generic_params =
                        super::callables::callable_generic_params(ctx, state, &decl.type_params);
                    super::type_impl_methods::inherit_owner_bounds(
                        ctx,
                        state,
                        self_type,
                        &owner,
                        &mut generic_params,
                        decl.span,
                    );
                    for spec_ref in specs {
                        if let ResolveResult::Resolved(spec_symbol) = spec_ref.spec_path.resolution
                        {
                            let spec_symbol = ctx
                                .symbols
                                .canonical_symbol(ctx.hir, spec_symbol)
                                .unwrap_or(spec_symbol);
                            let methods = if spec_signature(ctx, state, spec_symbol)
                                .is_some_and(|signature| !signature.methods.is_empty())
                            {
                                impl_methods.clone()
                            } else {
                                Vec::new()
                            };
                            state.spec_impls.push(SpecImplFact {
                                implementation: Some(etas_hir::HirItemId(impl_group)),
                                generic_params: generic_params.clone(),
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
                            if generic_params.is_empty()
                                && spec_signature(ctx, state, spec_symbol).is_some_and(
                                    |signature| matches!(signature.kind, SpecKind::TypeSpec),
                                )
                            {
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
    state: &SignaturePipelineState,
    signature: &etas_hir::HirFlowSignature,
) -> CallableSignature {
    CallableSignature {
        parameter_interface: Some(crate::call_interface::source_parameters(
            ctx.hir,
            &signature.params,
        )),
        generic_params: super::callables::callable_generic_params(
            ctx,
            state,
            &signature.type_params,
        ),
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
        parameter_interface: None,
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
    state: &SignaturePipelineState,
    flow: &etas_hir::HirFlowDecl,
) -> CallableSignature {
    CallableSignature {
        parameter_interface: Some(crate::call_interface::source_parameters(
            ctx.hir,
            &flow.params,
        )),
        generic_params: super::callables::callable_generic_params(ctx, state, &flow.type_params),
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
            let ty = ctx.interner.intern(crate::Type::Named(crate::NamedTypeRef {
                source_parameter: Some(symbol.id),
                name: symbol.name.clone(),
            }));
            state
                .symbol_types
                .insert(symbol.id, SymbolTypeFact::Param { ty });
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
