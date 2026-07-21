use etas_core::{Diagnostic, TypeDiagnosticCode};
use etas_hir::{
    HirEffectArg, HirEffectRef, HirEffectRow, HirItem, HirType, PartialResolutionReason,
    ResolveResult, SymbolDef, SymbolId, resolved_static_type_selector_symbol,
    static_selector_path_segments,
};

use crate::{
    EffectActionArgKind, EffectArgRef, EffectRef, EffectRowRef, NamedTypeRef, SymbolTypeFact, Type,
    TypeId, pipeline::context::TypePipelineContext,
};

pub fn lower_effect_row(ctx: &mut TypePipelineContext<'_>, row: &HirEffectRow) -> EffectRowRef {
    EffectRowRef {
        effects: row
            .effects
            .iter()
            .filter_map(|effect| lower_effect_ref(ctx, effect))
            .collect(),
        tail: row
            .tail
            .as_ref()
            .and_then(|tail| tail.segments.last().map(|segment| segment.name.clone())),
    }
}

fn lower_effect_ref(ctx: &mut TypePipelineContext<'_>, effect: &HirEffectRef) -> Option<EffectRef> {
    let name = effect
        .path
        .segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<Vec<_>>()
        .join(".");
    let action_arg_kinds = action_arg_kinds_for_effect_ref(ctx, effect);
    let is_error_effect = name == "Error"
        || name.rsplit('.').next() == Some("Error")
        || name.rsplit('.').next() == Some("raise");
    let args = effect
        .args
        .iter()
        .enumerate()
        .map(|(index, arg)| {
            lower_effect_arg(
                ctx,
                arg,
                action_arg_kinds.as_ref().and_then(|kinds| kinds.get(index)),
                is_error_effect,
            )
        })
        .collect::<Option<Vec<_>>>()?;
    Some(EffectRef { name, args })
}

fn lower_effect_arg(
    ctx: &mut TypePipelineContext<'_>,
    arg: &HirEffectArg,
    expected_kind: Option<&EffectActionArgKind>,
    is_error_effect: bool,
) -> Option<EffectArgRef> {
    match arg {
        HirEffectArg::Type(ty) => {
            if matches!(
                expected_kind,
                Some(
                    EffectActionArgKind::MemoryPlace
                        | EffectActionArgKind::StaticResourcePath { .. }
                        | EffectActionArgKind::StringPattern
                )
            ) {
                return match static_resource_path_from_type_ref(ctx, *ty) {
                    Some(segments) => Some(EffectArgRef::Path(segments)),
                    None => {
                        if let Some(span) = ctx.hir.types.get(*ty).map(|ty| ty.span()) {
                            invalid_static_effect_arg(
                                ctx,
                                span,
                                "effect action argument must be a static selector path for this action descriptor",
                            );
                        }
                        None
                    }
                };
            }
            let lowered = crate::lower::type_ref::lower_type_ref(ctx, *ty);
            if lowered.is_none()
                && let Some(span) = ctx.hir.types.get(*ty).map(|ty| ty.span())
            {
                invalid_static_effect_arg(ctx, span, "effect type argument must name a type");
            }
            lowered.map(EffectArgRef::Type)
        }
        HirEffectArg::Path(path)
            if is_error_effect || matches!(expected_kind, Some(EffectActionArgKind::Type)) =>
        {
            match type_arg_from_path(ctx, path) {
                Some(ty) => Some(EffectArgRef::Type(ty)),
                None => {
                    invalid_static_effect_arg(
                        ctx,
                        path.span,
                        "effect type argument must name a type",
                    );
                    None
                }
            }
        }
        HirEffectArg::Path(path) => {
            if expected_kind.is_none()
                && let Some(ty) = type_arg_from_path(ctx, path)
            {
                Some(EffectArgRef::Type(ty))
            } else {
                match static_resource_path_segments(ctx, path) {
                    Some(segments) => Some(EffectArgRef::Path(segments)),
                    None => {
                        invalid_static_effect_arg(
                            ctx,
                            path.span,
                            "effect action argument must be a static selector, literal, type argument, or `_`; runtime values belong in the action payload",
                        );
                        None
                    }
                }
            }
        }
        HirEffectArg::Wildcard { .. } => Some(EffectArgRef::Wildcard),
        HirEffectArg::String { value, .. } => Some(EffectArgRef::String(value.clone())),
        HirEffectArg::Int { text, .. } => Some(EffectArgRef::Int(text.clone())),
    }
}

fn action_arg_kinds_for_effect_ref(
    ctx: &TypePipelineContext<'_>,
    effect: &HirEffectRef,
) -> Option<Vec<EffectActionArgKind>> {
    if let Some(symbol) = action_symbol_for_effect_ref(ctx, effect) {
        if let Some(signature) = ctx.signature_facts.action_signatures.get(&symbol) {
            return Some(signature.effect_args.clone());
        }
        if let Some(SymbolTypeFact::EffectAction { signature }) =
            ctx.symbols
                .symbol_fact(ctx.hir, &ctx.signature_facts, symbol)
        {
            return Some(signature.effect_args.clone());
        }
    }
    let qualified = effect
        .path
        .segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<Vec<_>>()
        .join(".");
    if let Some(signature) = ctx
        .signature_facts
        .qualified_action_signatures
        .get(&qualified)
    {
        return Some(signature.effect_args.clone());
    }
    None
}

fn action_symbol_for_effect_ref(
    ctx: &TypePipelineContext<'_>,
    effect: &HirEffectRef,
) -> Option<etas_hir::SymbolId> {
    if let ResolveResult::Resolved(symbol) = effect.path.resolution {
        let symbol = ctx
            .symbols
            .canonical_symbol(ctx.hir, symbol)
            .unwrap_or(symbol);
        if ctx
            .signature_facts
            .symbol_types
            .get(&symbol)
            .is_some_and(|fact| matches!(fact, SymbolTypeFact::EffectAction { .. }))
        {
            return Some(symbol);
        }
    }
    if let ResolveResult::PartiallyResolved(partial) = &effect.path.resolution
        && partial.reason == PartialResolutionReason::MemberRequiresTypeChecking
        && partial.remaining.len() == 1
        && let Some(effect_symbol) = partial.resolved_prefix
    {
        let effect_symbol = ctx
            .symbols
            .canonical_symbol(ctx.hir, effect_symbol)
            .unwrap_or(effect_symbol);
        return action_symbol_for_effect_member(ctx, effect_symbol, &partial.remaining[0]);
    }
    None
}

fn action_symbol_for_effect_member(
    ctx: &TypePipelineContext<'_>,
    effect_symbol: SymbolId,
    action_name: &str,
) -> Option<SymbolId> {
    ctx.hir.items.iter().find_map(|(_, item)| {
        let HirItem::Effect(effect) = item else {
            return None;
        };
        let owner_symbol = ctx
            .symbols
            .canonical_symbol(ctx.hir, effect.symbol)
            .unwrap_or(effect.symbol);
        if owner_symbol != effect_symbol {
            return None;
        }
        effect.body.actions().iter().find_map(|action| {
            let action_symbol = ctx
                .symbols
                .canonical_symbol(ctx.hir, action.symbol)
                .unwrap_or(action.symbol);
            let symbol = ctx.hir.symbols.get(action_symbol)?;
            (symbol.name.rsplit('.').next() == Some(action_name)).then_some(action_symbol)
        })
    })
}

fn static_resource_path_segments(
    ctx: &TypePipelineContext<'_>,
    path: &etas_hir::ResolvedPath,
) -> Option<Vec<String>> {
    static_selector_path_segments(ctx.hir, path, Some)
}

fn static_resource_path_from_type_ref(
    ctx: &TypePipelineContext<'_>,
    ty: etas_hir::HirTypeId,
) -> Option<Vec<String>> {
    let HirType::Path { path, .. } = ctx.hir.types.get(ty)? else {
        return None;
    };
    static_resource_path_segments(ctx, path)
}

fn type_arg_from_path(
    ctx: &mut TypePipelineContext<'_>,
    path: &etas_hir::ResolvedPath,
) -> Option<TypeId> {
    let symbol = resolved_static_type_selector_symbol(path, Some)?;
    if let Some(symbol_data) = ctx.hir.symbols.get(symbol)
        && let SymbolDef::TypeParam { .. } = symbol_data.def
    {
        return Some(ctx.interner.intern(Type::Named(NamedTypeRef {
            name: symbol_data.name.clone(),
        })));
    }
    ctx.symbols
        .symbol_fact(ctx.hir, &ctx.signature_facts, symbol)
        .and_then(type_from_symbol_fact)
}

fn invalid_static_effect_arg(
    ctx: &mut TypePipelineContext<'_>,
    span: etas_core::Span,
    message: impl Into<String>,
) {
    ctx.diagnostics.push(Diagnostic::type_check(
        TypeDiagnosticCode::InvalidEffectArgument,
        span,
        message,
    ));
}

fn type_from_symbol_fact(fact: &SymbolTypeFact) -> Option<TypeId> {
    match fact {
        SymbolTypeFact::TypeAlias { target, .. } => Some(*target),
        SymbolTypeFact::Type { constructor } | SymbolTypeFact::NominalType { constructor, .. } => {
            Some(TypeId(constructor.0))
        }
        _ => None,
    }
}
