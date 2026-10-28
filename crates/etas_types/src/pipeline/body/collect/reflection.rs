use etas_hir::{HirExprId, HirReflectExpr, HirReflectTarget, ResolveResult, SymbolDef, SymbolKind};

use crate::{
    ConstraintOrigin, SymbolTypeFact, TypeConstraint, TypeId,
    constraint::reflection::ReflectionBinding, pipeline::context::BodyCollectContext,
};

pub(super) fn collect_reflection<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    expr: HirExprId,
    reflection: &HirReflectExpr,
) -> TypeId {
    let (binding, members) = match &reflection.target {
        HirReflectTarget::Path(path) => {
            let (symbol, members) = match &path.resolution {
                ResolveResult::Resolved(symbol) => (Some(*symbol), Vec::new()),
                ResolveResult::PartiallyResolved(partial) => {
                    (partial.resolved_prefix, partial.remaining.clone())
                }
                _ => (None, Vec::new()),
            };
            (
                symbol
                    .map(|symbol| binding_for_symbol(ctx, symbol))
                    .unwrap_or_else(|| ReflectionBinding::Invalid {
                        reason: "reflection target is not uniquely resolved".into(),
                    }),
                members,
            )
        }
        HirReflectTarget::SpecMethod {
            receiver,
            spec,
            spec_args,
            method,
            ..
        } => (
            spec_method_binding(ctx, receiver, spec, spec_args, method),
            Vec::new(),
        ),
    };
    let output = ctx.fresh_type_var();
    ctx.emit(TypeConstraint::Reflection {
        expr,
        access_module: ctx.ctx.hir.item_module(ctx.state.item),
        binding,
        members,
        output,
        origin: ConstraintOrigin {
            span: reflection.span,
        },
    });
    output
}

fn spec_method_binding<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    receiver: &etas_hir::ResolvedPath,
    spec: &etas_hir::ResolvedPath,
    spec_args: &[etas_hir::HirTypeId],
    method: &str,
) -> ReflectionBinding {
    let ResolveResult::Resolved(receiver) = receiver.resolution else {
        return ReflectionBinding::Invalid {
            reason: "spec reflection receiver is not uniquely resolved".into(),
        };
    };
    let ResolveResult::Resolved(spec) = spec.resolution else {
        return ReflectionBinding::Invalid {
            reason: "spec reflection must name a uniquely resolved spec".into(),
        };
    };
    let Some(spec) = ctx.ctx.symbols.canonical_symbol(ctx.ctx.hir, spec) else {
        return ReflectionBinding::Invalid {
            reason: "spec reflection contains a cyclic import binding".into(),
        };
    };
    let mut args = Vec::with_capacity(spec_args.len());
    for arg in spec_args {
        let Some(ty) = crate::lower::type_ref::lower_type_ref(ctx.ctx, *arg) else {
            return ReflectionBinding::Invalid {
                reason: "spec reflection has an unresolved type argument".into(),
            };
        };
        args.push(ty);
    }
    ReflectionBinding::SpecMethod {
        receiver: Box::new(binding_for_symbol(ctx, receiver)),
        spec,
        args,
        method: method.into(),
    }
}

fn binding_for_symbol<E>(
    ctx: &BodyCollectContext<'_, '_, E>,
    symbol: etas_hir::SymbolId,
) -> ReflectionBinding {
    let Some(canonical) = ctx.ctx.symbols.canonical_symbol(ctx.ctx.hir, symbol) else {
        return ReflectionBinding::Invalid {
            reason: "reflection target contains a cyclic import binding".into(),
        };
    };
    let fact = ctx
        .state
        .provisional
        .symbol_types
        .get(&canonical)
        .or_else(|| ctx.ctx.symbol_fact(canonical));
    let facts = ctx
        .ctx
        .signature_base
        .map_or(&ctx.ctx.signature_facts, |base| &base.facts);
    if facts.agent_methods.contains_key(&canonical) {
        return ReflectionBinding::AgentMethod(canonical);
    }
    match fact {
        Some(SymbolTypeFact::Agent { .. }) => ReflectionBinding::AgentDeclaration(canonical),
        Some(SymbolTypeFact::Flow { .. }) if is_flow_declaration(ctx, canonical) => {
            ReflectionBinding::CallableDeclaration(canonical)
        }
        Some(
            SymbolTypeFact::Param { ty }
            | SymbolTypeFact::Local { ty, .. }
            | SymbolTypeFact::Value { ty }
            | SymbolTypeFact::TopLevelLet { ty, .. },
        ) => ReflectionBinding::Value {
            binding: canonical,
            ty: *ty,
        },
        _ => ReflectionBinding::Invalid {
            reason: "reflection target is not an agent binding or a flow declaration".into(),
        },
    }
}

fn is_flow_declaration<E>(ctx: &BodyCollectContext<'_, '_, E>, symbol: etas_hir::SymbolId) -> bool {
    let Some(symbol) = ctx.ctx.hir.symbols.get(symbol) else {
        return false;
    };
    match symbol.kind {
        SymbolKind::Flow => true,
        SymbolKind::Import | SymbolKind::StdPreludeAlias => {
            let SymbolDef::ImportAlias { path, origin } = &symbol.def else {
                return false;
            };
            if *origin == etas_hir::ImportAliasOrigin::StdPrelude
                || path.first().is_some_and(|part| part == "std")
            {
                ctx.ctx
                    .std_registry
                    .lookup_qualified(path)
                    .is_some_and(|symbol| symbol.kind == etas_std::StdSymbolKind::Flow)
            } else {
                *origin == etas_hir::ImportAliasOrigin::SourceImport
            }
        }
        _ => false,
    }
}
