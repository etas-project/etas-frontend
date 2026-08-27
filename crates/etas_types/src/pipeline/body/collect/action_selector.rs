use etas_hir::{
    HirEffectArg, HirGenericArg, HirType, SymbolDef, resolved_static_type_selector_symbol,
    static_selector_path_segments,
};

use crate::{
    EffectActionArgKind, EffectActionSignature, EffectArgRef, NamedTypeRef, SymbolTypeFact, Type,
    TypeId, pipeline::context::BodyCollectContext,
};

pub fn validate_action_selector_arity_and_kinds(
    ctx: &mut BodyCollectContext<'_, '_>,
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

pub fn specialize_action_signature(
    ctx: &mut BodyCollectContext<'_, '_>,
    signature: EffectActionSignature,
    args: &[HirEffectArg],
) -> EffectActionSignature {
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
    if substitutions.is_empty() {
        return EffectActionSignature {
            generic_params: signature.generic_params,
            params: signature.params,
            output: signature.output,
            effect_args: signature.effect_args,
            selector_param_names: signature.selector_param_names,
            selector_defaults,
            returns_never: signature.returns_never,
        };
    }
    EffectActionSignature {
        generic_params: signature.generic_params,
        params: signature
            .params
            .into_iter()
            .map(|ty| substitute_schematic_type(ctx, ty, &substitutions))
            .collect(),
        output: substitute_schematic_type(ctx, signature.output, &substitutions),
        effect_args: signature.effect_args,
        selector_param_names: signature.selector_param_names,
        selector_defaults,
        returns_never: signature.returns_never,
    }
}

pub fn specialize_action_signature_from_generic_args(
    ctx: &mut BodyCollectContext<'_, '_>,
    signature: EffectActionSignature,
    generic_args: &[HirGenericArg],
) -> EffectActionSignature {
    if generic_args.is_empty() {
        return signature;
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
    if substitutions.is_empty() {
        return EffectActionSignature {
            generic_params: signature.generic_params,
            params: signature.params,
            output: signature.output,
            effect_args: signature.effect_args,
            selector_param_names: signature.selector_param_names,
            selector_defaults,
            returns_never: signature.returns_never,
        };
    }
    EffectActionSignature {
        generic_params: signature.generic_params,
        params: signature
            .params
            .into_iter()
            .map(|ty| substitute_schematic_type(ctx, ty, &substitutions))
            .collect(),
        output: substitute_schematic_type(ctx, signature.output, &substitutions),
        effect_args: signature.effect_args,
        selector_param_names: signature.selector_param_names,
        selector_defaults,
        returns_never: signature.returns_never,
    }
}

fn effect_arg_selector_substitutions(
    ctx: &mut BodyCollectContext<'_, '_>,
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

fn generic_arg_selector_substitutions(
    ctx: &mut BodyCollectContext<'_, '_>,
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

fn selector_arg_matches_kind(
    ctx: &mut BodyCollectContext<'_, '_>,
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

fn generic_arg_default(
    ctx: &mut BodyCollectContext<'_, '_>,
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

fn generic_arg_type(ctx: &mut BodyCollectContext<'_, '_>, arg: &HirGenericArg) -> Option<TypeId> {
    match arg {
        HirGenericArg::Type(ty) => static_type_selector(ctx, *ty),
        HirGenericArg::Wildcard { .. } | HirGenericArg::EffectRow(_) => None,
    }
}

fn generic_arg_span(
    ctx: &BodyCollectContext<'_, '_>,
    arg: &HirGenericArg,
) -> Option<etas_core::Span> {
    match arg {
        HirGenericArg::Type(ty) => Some(ctx.ctx.hir.types.get(*ty)?.span()),
        HirGenericArg::EffectRow(row) => Some(row.span),
        HirGenericArg::Wildcard { span } => Some(*span),
    }
}

fn static_type_selector(
    ctx: &mut BodyCollectContext<'_, '_>,
    ty: etas_hir::HirTypeId,
) -> Option<TypeId> {
    match ctx.ctx.hir.types.get(ty)?.clone() {
        HirType::Path { path, .. } => type_arg_from_path(ctx, &path),
        _ => crate::lower::type_ref::lower_type_ref(ctx.ctx, ty),
    }
}

fn static_path_selector(ctx: &BodyCollectContext<'_, '_>, ty: etas_hir::HirTypeId) -> bool {
    let Some(HirType::Path { path, .. }) = ctx.ctx.hir.types.get(ty) else {
        return false;
    };
    static_selector_path_segments(ctx.ctx.hir, path, |symbol| {
        ctx.ctx.symbols.canonical_symbol(ctx.ctx.hir, symbol)
    })
    .is_some()
}

fn static_path_arg_from_type(
    ctx: &BodyCollectContext<'_, '_>,
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

fn static_memory_place_selector(
    ctx: &mut BodyCollectContext<'_, '_>,
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

fn memory_place_arg_from_type(
    ctx: &mut BodyCollectContext<'_, '_>,
    ty: etas_hir::HirTypeId,
) -> Option<EffectArgRef> {
    if let Some(type_id) = crate::lower::type_ref::lower_type_ref(ctx.ctx, ty)
        && let Some(Type::MemoryPlace(place)) = ctx.ctx.interner.store().get(type_id)
    {
        return Some(EffectArgRef::Path(place.segments.clone()));
    }
    static_path_arg_from_type(ctx, ty)
}

fn effect_arg_default(
    ctx: &mut BodyCollectContext<'_, '_>,
    arg: &HirEffectArg,
    kind: &EffectActionArgKind,
) -> Option<EffectArgRef> {
    lower_selector_effect_arg(ctx, arg, kind)
}

fn lower_selector_effect_arg(
    ctx: &mut BodyCollectContext<'_, '_>,
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

fn effect_arg_type(ctx: &mut BodyCollectContext<'_, '_>, arg: &HirEffectArg) -> Option<TypeId> {
    match arg {
        HirEffectArg::Type(ty) => crate::lower::type_ref::lower_type_ref(ctx.ctx, *ty),
        HirEffectArg::Path(path) => type_arg_from_path(ctx, path),
        _ => None,
    }
}

fn static_resource_path_arg(
    ctx: &BodyCollectContext<'_, '_>,
    path: &etas_hir::ResolvedPath,
) -> Option<EffectArgRef> {
    static_selector_path_segments(ctx.ctx.hir, path, |symbol| {
        ctx.ctx.symbols.canonical_symbol(ctx.ctx.hir, symbol)
    })
    .map(EffectArgRef::Path)
}

fn type_arg_from_path(
    ctx: &mut BodyCollectContext<'_, '_>,
    path: &etas_hir::ResolvedPath,
) -> Option<TypeId> {
    let symbol = resolved_static_type_selector_symbol(path, |symbol| {
        ctx.ctx.symbols.canonical_symbol(ctx.ctx.hir, symbol)
    })?;
    if let Some(symbol_data) = ctx.ctx.hir.symbols.get(symbol)
        && let SymbolDef::TypeParam { .. } = symbol_data.def
    {
        return Some(ctx.ctx.interner.intern(Type::Named(NamedTypeRef {
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
        .or_else(|| {
            ctx.ctx
                .symbols
                .symbol_fact(ctx.ctx.hir, &ctx.ctx.signature_facts, symbol)
        })
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

fn substitute_schematic_type(
    ctx: &mut BodyCollectContext<'_, '_>,
    ty: TypeId,
    substitutions: &std::collections::HashMap<String, TypeId>,
) -> TypeId {
    let Some(ty_data) = ctx.ctx.interner.store().get(ty).cloned() else {
        return ty;
    };
    match ty_data {
        Type::Named(name) if is_schematic_type_variable(&name.name) => {
            substitutions.get(&name.name).copied().unwrap_or(ty)
        }
        Type::Array(inner) => {
            let inner = substitute_schematic_type(ctx, inner, substitutions);
            ctx.ctx.interner.intern(Type::Array(inner))
        }
        Type::List(inner) => {
            let inner = substitute_schematic_type(ctx, inner, substitutions);
            ctx.ctx.interner.intern(Type::List(inner))
        }
        Type::Set(inner) => {
            let inner = substitute_schematic_type(ctx, inner, substitutions);
            ctx.ctx.interner.intern(Type::Set(inner))
        }
        Type::Slice(inner) => {
            let inner = substitute_schematic_type(ctx, inner, substitutions);
            ctx.ctx.interner.intern(Type::Slice(inner))
        }
        Type::Option(inner) => {
            let inner = substitute_schematic_type(ctx, inner, substitutions);
            ctx.ctx.interner.intern(Type::Option(inner))
        }
        Type::Result { ok, err } => {
            let ok = substitute_schematic_type(ctx, ok, substitutions);
            let err = substitute_schematic_type(ctx, err, substitutions);
            ctx.ctx.interner.intern(Type::Result { ok, err })
        }
        Type::Function(mut flow) => {
            flow.input = flow
                .input
                .into_iter()
                .map(|ty| substitute_schematic_type(ctx, ty, substitutions))
                .collect();
            flow.output = substitute_schematic_type(ctx, flow.output, substitutions);
            ctx.ctx.interner.intern(Type::Function(flow))
        }
        Type::MemoryRegion(inner) => {
            let inner = substitute_schematic_type(ctx, inner, substitutions);
            ctx.ctx.interner.intern(Type::MemoryRegion(inner))
        }
        Type::ResourceHandle(crate::ResourceHandleType::MemoryRegion { schema }) => {
            let schema = substitute_schematic_type(ctx, schema, substitutions);
            ctx.ctx.interner.intern(Type::ResourceHandle(
                crate::ResourceHandleType::MemoryRegion { schema },
            ))
        }
        _ => ty,
    }
}

fn is_schematic_type_variable(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(ch) if ch.is_ascii_uppercase()) && chars.next().is_none()
}
