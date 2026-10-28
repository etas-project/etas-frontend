use etas_hir::{
    HirEffectArg, HirGenericArg, HirType, SymbolDef, resolved_static_type_selector_symbol,
    static_selector_path_segments,
};

use crate::{
    EffectActionArgKind, EffectActionSignature, EffectArgRef, NamedTypeRef, SymbolTypeFact, Type,
    TypeId, pipeline::context::BodyCollectContext,
};

pub fn validate_action_selector_arity_and_kinds<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    signature: &EffectActionSignature,
    generic_args: &[HirGenericArg],
    span: etas_core::Span,
    site: &str,
) {
    let expected = signature.effect_args.len();
    if !generic_args.is_empty() && generic_args.len() != expected {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: etas_core::TypeDiagnosticCode::InvalidEffectArgument,
            span,
            message: format!(
                "{site} selector expects {expected} static argument(s), got {}",
                generic_args.len()
            ),
        });
        return;
    }
    if generic_args.is_empty() {
        return;
    }
    for (kind, arg) in signature.effect_args.iter().zip(generic_args) {
        if !selector_arg_matches_kind(ctx, kind, arg) {
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::InvalidEffectArgument,
                span: generic_arg_span(ctx, arg).unwrap_or(span),
                message: format!(
                    "{site} selector argument must be static and match the action descriptor kind"
                ),
            });
        }
    }
}

pub fn specialize_action_signature<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    signature: EffectActionSignature,
    args: &[HirEffectArg],
    span: etas_core::Span,
) -> Option<EffectActionSignature> {
    let mut selector_defaults = signature.selector_defaults.clone();
    if selector_defaults.len() < signature.effect_args.len() {
        selector_defaults.resize(signature.effect_args.len(), None);
    }
    for (index, (arg, kind)) in args.iter().zip(signature.effect_args.iter()).enumerate() {
        if let Some(default) = effect_arg_default(ctx, arg, kind) {
            selector_defaults[index] = Some(default);
        }
    }
    let substitutions = effect_arg_selector_substitutions(ctx, &signature, args);
    apply_substitutions(ctx, signature, selector_defaults, &substitutions, span)
}

pub fn specialize_action_signature_from_generic_args<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    signature: EffectActionSignature,
    generic_args: &[HirGenericArg],
    span: etas_core::Span,
) -> Option<EffectActionSignature> {
    if generic_args.is_empty() {
        return Some(signature);
    }
    let mut selector_defaults = signature.selector_defaults.clone();
    if selector_defaults.len() < signature.effect_args.len() {
        selector_defaults.resize(signature.effect_args.len(), None);
    }
    for (index, (arg, kind)) in generic_args
        .iter()
        .zip(signature.effect_args.iter())
        .enumerate()
    {
        if let Some(default) = generic_arg_default(ctx, arg, kind) {
            selector_defaults[index] = Some(default);
        }
    }
    let substitutions = generic_arg_selector_substitutions(ctx, &signature, generic_args);
    apply_substitutions(ctx, signature, selector_defaults, &substitutions, span)
}

pub fn checked_selector_arguments(
    signature: &EffectActionSignature,
    explicit_count: usize,
) -> Option<Vec<EffectArgRef>> {
    if explicit_count != 0 && explicit_count != signature.effect_args.len() {
        return None;
    }
    signature
        .effect_args
        .iter()
        .enumerate()
        .map(
            |(index, _)| match signature.selector_defaults.get(index)?.as_ref() {
                Some(arg) => Some(arg.clone()),
                None if explicit_count == 0 => Some(EffectArgRef::Wildcard),
                None => None,
            },
        )
        .collect()
}

pub(super) fn apply_substitutions<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    mut signature: EffectActionSignature,
    selector_defaults: Vec<Option<EffectArgRef>>,
    substitutions: &std::collections::HashMap<String, TypeId>,
    span: etas_core::Span,
) -> Option<EffectActionSignature> {
    let result = (|| {
        let mut engine = crate::TypeSubstitutionEngine::materializing(
            &mut ctx.ctx.interner,
            substitutions,
            None,
        );
        signature.params = signature
            .params
            .into_iter()
            .map(|ty| engine.substitute(ty))
            .collect::<Result<_, _>>()?;
        signature.output = engine.substitute(signature.output)?;
        signature.default_handler_effects = signature
            .default_handler_effects
            .map(|row| engine.substitute_effect_row(row))
            .transpose()?;
        signature.selector_defaults = selector_defaults;
        Ok::<_, crate::TypeSubstitutionError>(signature)
    })();
    match result {
        Ok(signature) => Some(signature),
        Err(error) => {
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span,
                message: format!("cannot specialize the checked action signature: {error}"),
            });
            None
        }
    }
}

fn effect_arg_selector_substitutions<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    signature: &EffectActionSignature,
    args: &[HirEffectArg],
) -> std::collections::HashMap<String, TypeId> {
    let mut substitutions = std::collections::HashMap::new();
    for (index, (arg, kind)) in args.iter().zip(signature.effect_args.iter()).enumerate() {
        if !matches!(kind, EffectActionArgKind::Type) {
            continue;
        }
        let Some(name) = signature.selector_param_names.get(index) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        if let Some(ty) = effect_arg_type(ctx, arg) {
            substitutions.insert(name.clone(), ty);
        }
    }
    substitutions
}

fn generic_arg_selector_substitutions<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    signature: &EffectActionSignature,
    args: &[HirGenericArg],
) -> std::collections::HashMap<String, TypeId> {
    let mut substitutions = std::collections::HashMap::new();
    for (index, (arg, kind)) in args.iter().zip(signature.effect_args.iter()).enumerate() {
        if !matches!(kind, EffectActionArgKind::Type) {
            continue;
        }
        let Some(name) = signature.selector_param_names.get(index) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        if let Some(ty) = generic_arg_type(ctx, arg) {
            substitutions.insert(name.clone(), ty);
        }
    }
    substitutions
}

fn selector_arg_matches_kind<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    kind: &EffectActionArgKind,
    arg: &HirGenericArg,
) -> bool {
    match kind {
        EffectActionArgKind::Type => match arg {
            HirGenericArg::Type(ty) => static_type_selector(ctx, *ty).is_some(),
            HirGenericArg::Wildcard { .. } => true,
            HirGenericArg::EffectRow(_) => false,
        },
        EffectActionArgKind::MemoryPlace => match arg {
            HirGenericArg::Type(ty) => static_memory_place_selector(ctx, *ty),
            HirGenericArg::Wildcard { .. } => true,
            HirGenericArg::EffectRow(_) => false,
        },
        EffectActionArgKind::StaticResourcePath { .. } => match arg {
            HirGenericArg::Type(ty) => static_path_selector(ctx, *ty),
            HirGenericArg::Wildcard { .. } => true,
            HirGenericArg::EffectRow(_) => false,
        },
        EffectActionArgKind::StringPattern => match arg {
            HirGenericArg::Type(ty) => static_path_selector(ctx, *ty),
            HirGenericArg::Wildcard { .. } => true,
            HirGenericArg::EffectRow(_) => false,
        },
    }
}

fn generic_arg_default<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    arg: &HirGenericArg,
    kind: &EffectActionArgKind,
) -> Option<EffectArgRef> {
    match (kind, arg) {
        (_, HirGenericArg::Wildcard { .. }) => Some(EffectArgRef::Wildcard),
        (EffectActionArgKind::Type, HirGenericArg::Type(ty)) => {
            static_type_selector(ctx, *ty).map(EffectArgRef::Type)
        }
        (EffectActionArgKind::MemoryPlace, HirGenericArg::Type(ty)) => {
            memory_place_arg_from_type(ctx, *ty)
        }
        (EffectActionArgKind::StaticResourcePath { .. }, HirGenericArg::Type(ty))
        | (EffectActionArgKind::StringPattern, HirGenericArg::Type(ty)) => {
            static_path_arg_from_type(ctx, *ty)
        }
        (_, HirGenericArg::EffectRow(_)) => None,
    }
}

fn generic_arg_type<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    arg: &HirGenericArg,
) -> Option<TypeId> {
    match arg {
        HirGenericArg::Type(ty) => static_type_selector(ctx, *ty),
        HirGenericArg::Wildcard { .. } | HirGenericArg::EffectRow(_) => None,
    }
}

fn generic_arg_span<E>(
    ctx: &BodyCollectContext<'_, '_, E>,
    arg: &HirGenericArg,
) -> Option<etas_core::Span> {
    match arg {
        HirGenericArg::Type(ty) => Some(ctx.ctx.hir.types.get(*ty)?.span()),
        HirGenericArg::EffectRow(row) => Some(row.span),
        HirGenericArg::Wildcard { span } => Some(*span),
    }
}

fn static_type_selector<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    ty: etas_hir::HirTypeId,
) -> Option<TypeId> {
    match ctx.ctx.hir.types.get(ty)?.clone() {
        HirType::Path { path, args, .. } => {
            let declaration = type_arg_from_path(ctx, &path)?;
            if args.is_empty() {
                Some(declaration)
            } else {
                crate::lower::type_ref::lower_type_ref(ctx.ctx, ty)
            }
        }
        _ => crate::lower::type_ref::lower_type_ref(ctx.ctx, ty),
    }
}

fn static_path_selector<E>(ctx: &BodyCollectContext<'_, '_, E>, ty: etas_hir::HirTypeId) -> bool {
    let Some(HirType::Path { path, .. }) = ctx.ctx.hir.types.get(ty) else {
        return false;
    };
    static_selector_path_segments(ctx.ctx.hir, path, |symbol| {
        ctx.ctx.symbols.canonical_symbol(ctx.ctx.hir, symbol)
    })
    .is_some()
}

fn static_path_arg_from_type<E>(
    ctx: &BodyCollectContext<'_, '_, E>,
    ty: etas_hir::HirTypeId,
) -> Option<EffectArgRef> {
    let Some(HirType::Path { path, .. }) = ctx.ctx.hir.types.get(ty) else {
        return None;
    };
    static_selector_path_segments(ctx.ctx.hir, path, |symbol| {
        ctx.ctx.symbols.canonical_symbol(ctx.ctx.hir, symbol)
    })
    .map(EffectArgRef::Path)
}

fn static_memory_place_selector<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    ty: etas_hir::HirTypeId,
) -> bool {
    let Some(type_id) = crate::lower::type_ref::lower_type_ref(ctx.ctx, ty) else {
        return false;
    };
    matches!(
        ctx.ctx.interner.store().get(type_id),
        Some(Type::MemoryPlace(_))
    )
}

fn memory_place_arg_from_type<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    ty: etas_hir::HirTypeId,
) -> Option<EffectArgRef> {
    if let Some(type_id) = crate::lower::type_ref::lower_type_ref(ctx.ctx, ty)
        && let Some(Type::MemoryPlace(place)) = ctx.ctx.interner.store().get(type_id)
    {
        return Some(EffectArgRef::Path(place.segments.clone()));
    }
    static_path_arg_from_type(ctx, ty)
}

fn effect_arg_default<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    arg: &HirEffectArg,
    kind: &EffectActionArgKind,
) -> Option<EffectArgRef> {
    lower_selector_effect_arg(ctx, arg, kind)
}

fn lower_selector_effect_arg<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    arg: &HirEffectArg,
    kind: &EffectActionArgKind,
) -> Option<EffectArgRef> {
    if matches!(arg, HirEffectArg::Wildcard { .. }) {
        return Some(EffectArgRef::Wildcard);
    }
    match kind {
        EffectActionArgKind::Type => effect_arg_type(ctx, arg).map(EffectArgRef::Type),
        EffectActionArgKind::MemoryPlace => match arg {
            HirEffectArg::Path(path) => static_resource_path_arg(ctx, path),
            _ => None,
        },
        EffectActionArgKind::StaticResourcePath { .. } => match arg {
            HirEffectArg::Path(path) => static_resource_path_arg(ctx, path),
            _ => None,
        },
        EffectActionArgKind::StringPattern => match arg {
            HirEffectArg::String { value, .. } => Some(EffectArgRef::String(value.clone())),
            HirEffectArg::Int { text, .. } => Some(EffectArgRef::Int(text.clone())),
            HirEffectArg::Path(path) => static_resource_path_arg(ctx, path),
            _ => None,
        },
    }
}

fn effect_arg_type<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    arg: &HirEffectArg,
) -> Option<TypeId> {
    match arg {
        HirEffectArg::Type(ty) => crate::lower::type_ref::lower_type_ref(ctx.ctx, *ty),
        HirEffectArg::Path(path) => type_arg_from_path(ctx, path),
        _ => None,
    }
}

fn static_resource_path_arg<E>(
    ctx: &BodyCollectContext<'_, '_, E>,
    path: &etas_hir::ResolvedPath,
) -> Option<EffectArgRef> {
    static_selector_path_segments(ctx.ctx.hir, path, |symbol| {
        ctx.ctx.symbols.canonical_symbol(ctx.ctx.hir, symbol)
    })
    .map(EffectArgRef::Path)
}

fn type_arg_from_path<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    path: &etas_hir::ResolvedPath,
) -> Option<TypeId> {
    let symbol = resolved_static_type_selector_symbol(path, |symbol| {
        ctx.ctx.symbols.canonical_symbol(ctx.ctx.hir, symbol)
    })?;
    if let Some(symbol_data) = ctx.ctx.hir.symbols.get(symbol)
        && let SymbolDef::TypeParam { .. } = symbol_data.def
    {
        return Some(ctx.ctx.interner.intern(Type::Named(NamedTypeRef {
            source_parameter: Some(symbol),
            name: symbol_data.name.clone(),
        })));
    }
    let symbol = ctx
        .ctx
        .symbols
        .canonical_symbol(ctx.ctx.hir, symbol)
        .unwrap_or(symbol);
    ctx.state
        .provisional
        .symbol_types
        .get(&symbol)
        .or_else(|| ctx.ctx.symbol_fact(symbol))
        .and_then(type_from_symbol_fact)
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
