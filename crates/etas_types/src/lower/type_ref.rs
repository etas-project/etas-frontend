use etas_core::{Diagnostic, TypeDiagnosticCode};
use etas_hir::{
    HirItem, HirPrimitiveType, HirType, HirTypeDeclBody, HirTypeId, ResolveResult, SymbolDef,
};

use crate::{
    EnumTypeRef, FieldType, FlowType, HandlerProducedEffects, HandlerType, NamedTypeRef,
    NominalTypeRef, PrimitiveType, RecordType, TrustWrapper, Type, TypeId,
    pipeline::context::TypePipelineContext,
};

pub fn lower_type_ref(ctx: &mut TypePipelineContext<'_>, ty: HirTypeId) -> Option<TypeId> {
    let is_path_type = matches!(
        ctx.hir.types[ty],
        HirType::Path { .. } | HirType::Metadata { .. }
    );
    if !is_path_type && let Some(existing) = ctx.type_ref(ty) {
        return match ctx.instantiate_annotation_type(existing) {
            Ok(lowered) => {
                if lowered != existing {
                    ctx.signature_facts.type_refs.insert(ty, lowered);
                }
                Some(lowered)
            }
            Err(error) => {
                ctx.diagnostics.push(Diagnostic::type_check(
                    TypeDiagnosticCode::IncompleteTypeFacts,
                    ctx.hir.types[ty].span(),
                    format!("bound body annotation could not be instantiated: {error}"),
                ));
                Some(ctx.interner.primitive(PrimitiveType::Never))
            }
        };
    }
    let lowered = match ctx.hir.types[ty].clone() {
        HirType::Metadata {
            ty: template,
            parameters,
            span,
        } => {
            let input = match super::external::package_type_input(&template) {
                Ok(input) => input,
                Err(error) => {
                    ctx.diagnostics.push(Diagnostic::type_check(
                        TypeDiagnosticCode::IncompleteTypeFacts,
                        span,
                        error.message,
                    ));
                    return None;
                }
            };
            let lowered = super::external::lower_external_type(ctx, span, &input);
            let named = parameters
                .into_iter()
                .filter_map(|parameter| {
                    Some((
                        ctx.hir.symbols.get(parameter)?.name.clone(),
                        ctx.bound_type_parameter(parameter)?,
                    ))
                })
                .collect();
            let lowered =
                match crate::TypeSubstitutionEngine::materializing(&mut ctx.interner, &named, None)
                    .substitute(lowered)
                {
                    Ok(lowered) => lowered,
                    Err(error) => {
                        ctx.diagnostics.push(Diagnostic::type_check(
                            TypeDiagnosticCode::IncompleteTypeFacts,
                            span,
                            error.to_string(),
                        ));
                        return None;
                    }
                };
            match ctx.instantiate_annotation_type(lowered) {
                Ok(lowered) => lowered,
                Err(error) => {
                    ctx.diagnostics.push(Diagnostic::type_check(
                        TypeDiagnosticCode::IncompleteTypeFacts,
                        span,
                        error.to_string(),
                    ));
                    return None;
                }
            }
        }
        HirType::Existential { bounds, span } => {
            let bounds = super::existential::bounds(ctx, &bounds)?;
            let ty = ctx.interner.intern(Type::Existential { bounds });
            ctx.existential_annotations.push((ty, span));
            ty
        }
        HirType::Primitive { kind, .. } => ctx.interner.primitive(PrimitiveType::from_hir(kind)),
        HirType::Array { element, .. } => {
            let element = lower_type_ref(ctx, element)?;
            ctx.interner.intern(Type::Array(element))
        }
        HirType::Tuple { elems, .. } => {
            let elems = elems
                .into_iter()
                .filter_map(|elem| lower_type_ref(ctx, elem))
                .collect();
            ctx.interner.intern(Type::Tuple(elems))
        }
        HirType::Record { fields, span } => {
            let mut seen = std::collections::HashSet::new();
            let fields = fields
                .into_iter()
                .filter_map(|field| {
                    let symbol = ctx.hir.symbols.get(field.symbol)?;
                    if !seen.insert(symbol.name.clone()) {
                        ctx.diagnostics.push(Diagnostic::type_check(
                            TypeDiagnosticCode::DuplicateField,
                            field.span,
                            "record type declares the same field more than once",
                        ));
                    }
                    Some(FieldType {
                        name: symbol.name.clone(),
                        ty: lower_type_ref(ctx, field.ty)?,
                    })
                })
                .collect::<Vec<_>>();
            if fields.is_empty()
                && !matches!(ctx.hir.types[ty], HirType::Record { fields: ref f, .. } if f.is_empty())
            {
                ctx.unknown_type(span, "record field type could not be lowered")
            } else {
                ctx.interner.intern(Type::Record(RecordType { fields }))
            }
        }
        HirType::Path { path, args, span } => lower_path_type(ctx, &path, &args, span),
        HirType::Arrow {
            input,
            output,
            effect,
            ..
        } => {
            let input = lower_arrow_input(ctx, input)?;
            let output = lower_type_ref(ctx, output)?;
            let effects = effect
                .as_ref()
                .map(|row| crate::lower::effect_row::lower_effect_row(ctx, row));
            ctx.interner.intern(Type::Function(FlowType {
                input,
                output,
                effects,
            }))
        }
        HirType::Handler {
            handled,
            produced,
            result,
            ..
        } => {
            let handled = crate::lower::effect_row::lower_effect_row(ctx, &handled);
            let produced = match produced {
                etas_hir::HirHandlerProducedEffects::Infer => HandlerProducedEffects::Infer,
                etas_hir::HirHandlerProducedEffects::Explicit(row) => {
                    HandlerProducedEffects::Explicit(crate::lower::effect_row::lower_effect_row(
                        ctx, &row,
                    ))
                }
            };
            let result = result.and_then(|result| lower_type_ref(ctx, result));
            ctx.interner.intern(Type::Handler(HandlerType {
                handled,
                produced,
                result,
            }))
        }
        HirType::Refined { base, .. } => lower_type_ref(ctx, base)?,
        HirType::Error { span } => ctx.unknown_type(span, "error type cannot be checked"),
    };
    ctx.signature_facts.type_refs.insert(ty, lowered);
    Some(lowered)
}

/// Resolve a language dependency in the selected source std namespace. The
/// declaration and its signature come from source/compiled metadata, never a
/// handwritten replacement declaration or a legacy factory.
pub(crate) fn selected_source_type(
    ctx: &mut TypePipelineContext<'_>,
    path: &[&str],
) -> Option<TypeId> {
    let path = path
        .iter()
        .map(|part| (*part).to_owned())
        .collect::<Vec<_>>();
    ctx.symbols
        .canonical_path(ctx.hir, &path)
        .and_then(|symbol| ctx.type_fact_as_type_id(symbol))
        .or_else(|| ctx.external_type_paths.get(&path).copied())
        .or_else(|| {
            let registry = ctx.std_registry.clone();
            let declaration = registry.lookup_qualified(&path)?;
            crate::lower::std::lower_std_type_symbol(ctx, &registry, declaration)
        })
}

/// A constructor sum owns a nominal identity. Its empty/cell storage is a
/// language value layout, derived from the declaration rather than a std role.
pub(crate) fn lower_sequence_representation(
    ctx: &mut TypePipelineContext<'_>,
    owner: etas_hir::SymbolId,
    parameters: &[etas_hir::SymbolId],
    element: HirTypeId,
    tail: HirTypeId,
    span: etas_core::Span,
) -> Option<TypeId> {
    let canonical = |symbol| {
        ctx.symbols
            .canonical_symbol(ctx.hir, symbol)
            .unwrap_or(symbol)
    };
    let recursive_tail = match &ctx.hir.types[tail] {
        HirType::Path { path, args, .. } => {
            matches!(path.resolution, ResolveResult::Resolved(symbol)
                if canonical(symbol) == canonical(owner))
                && args.len() == parameters.len()
                && args.iter().zip(parameters).all(|(argument, parameter)| {
                    matches!(&ctx.hir.types[*argument],
                        HirType::Path { path, args, .. }
                        if args.is_empty() && matches!(path.resolution,
                            ResolveResult::Resolved(symbol)
                            if canonical(symbol) == canonical(*parameter)))
                })
        }
        _ => false,
    };
    if !recursive_tail {
        ctx.diagnostics.push(Diagnostic::type_check(
            TypeDiagnosticCode::IncompleteTypeFacts,
            span,
            "a cell constructor must end in its owning type with the same type parameters",
        ));
        return None;
    }
    let element = lower_type_ref(ctx, element)?;
    Some(ctx.interner.intern(Type::List(element)))
}

pub fn lower_path_type(
    ctx: &mut TypePipelineContext<'_>,
    path: &etas_hir::ResolvedPath,
    args: &[HirTypeId],
    span: etas_core::Span,
) -> TypeId {
    let name = path
        .segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<Vec<_>>()
        .join(".");
    let args = args
        .iter()
        .filter_map(|arg| {
            if let HirType::Path { path, args, .. } = &ctx.hir.types[*arg]
                && args.is_empty()
                && let ResolveResult::Resolved(symbol) = path.resolution
                && let Some(symbol) = ctx
                    .symbols
                    .canonical_symbol(ctx.hir, symbol)
                    .and_then(|id| ctx.hir.symbols.get(id))
                && symbol.kind == etas_hir::SymbolKind::EffectParam
            {
                let row = ctx
                    .bound_effect_parameter(symbol.id)
                    .cloned()
                    .unwrap_or_else(|| crate::EffectRowRef {
                        effects: vec![],
                        tail: Some(symbol.name.clone()),
                    });
                return Some(ctx.interner.intern(Type::Handler(HandlerType {
                    handled: row,
                    produced: HandlerProducedEffects::Infer,
                    result: None,
                })));
            }
            lower_type_ref(ctx, *arg)
        })
        .collect::<Vec<_>>();
    lower_named_path(ctx, path, &name, args, span)
}

fn lower_named_path(
    ctx: &mut TypePipelineContext<'_>,
    path: &etas_hir::ResolvedPath,
    name: &str,
    args: Vec<TypeId>,
    span: etas_core::Span,
) -> TypeId {
    if let Some(method) = agent_context_ref(ctx, path) {
        if !args.is_empty() {
            return ctx.unknown_type(span, "agent method context does not accept type arguments");
        }
        return ctx.interner.intern(Type::AgentContext(method));
    }
    let source_symbol = resolved_source_type_symbol(ctx, path);
    if let Some(symbol) = source_symbol {
        let canonical = ctx
            .symbols
            .canonical_symbol(ctx.hir, symbol)
            .unwrap_or(symbol);
        let is_source_type = ctx.hir.symbols.get(canonical).is_some_and(|symbol| {
            matches!(symbol.def, SymbolDef::Item { .. })
                && matches!(
                    symbol.kind,
                    etas_hir::SymbolKind::Type
                        | etas_hir::SymbolKind::TypeAlias
                        | etas_hir::SymbolKind::Enum
                        | etas_hir::SymbolKind::Agent
                )
        });
        if is_source_type
            || matches!(
                ctx.symbol_fact(symbol),
                Some(
                    crate::SymbolTypeFact::TypeAlias { .. }
                        | crate::SymbolTypeFact::NominalType { .. }
                        | crate::SymbolTypeFact::Agent { .. }
                        | crate::SymbolTypeFact::Type { .. }
                )
            )
        {
            ctx.type_applications
                .push(super::type_application::TypeApplication {
                    symbol,
                    arguments: args.clone(),
                    span,
                });
        }
    }
    let standard_name = match path.resolution {
        ResolveResult::Resolved(symbol) => ctx
            .symbols
            .canonical_symbol(ctx.hir, symbol)
            .and_then(|symbol| ctx.hir.symbols.get(symbol))
            .and_then(|symbol| match &symbol.def {
                SymbolDef::ImportAlias { path, .. } => ctx.std_registry.lookup_qualified(path),
                _ => ctx
                    .source_type_role(symbol.id)
                    .and_then(|native| ctx.std_registry.symbol(native)),
            })
            .and_then(|symbol| match &symbol.decl {
                etas_std::StdDecl::Type(decl) => Some(decl.name.clone()),
                _ => None,
            }),
        _ => None,
    };
    let legacy_standard_type = source_symbol
        .and_then(|symbol| ctx.symbols.canonical_symbol(ctx.hir, symbol))
        .and_then(|symbol| ctx.hir.symbols.get(symbol))
        .and_then(|symbol| match &symbol.def {
            SymbolDef::ImportAlias { path, .. } => ctx.std_registry.lookup_qualified(path),
            _ => None,
        })
        .is_some_and(|symbol| {
            matches!(symbol.decl, etas_std::StdDecl::Type(_))
                && ctx.std_registry.declaration_key(symbol.id).is_none()
        });
    let resolved_declaration = match source_symbol {
        Some(symbol) => {
            matches!(
                ctx.symbol_fact(symbol),
                Some(
                    crate::SymbolTypeFact::TypeAlias { .. }
                        | crate::SymbolTypeFact::NominalType { .. }
                        | crate::SymbolTypeFact::Agent { .. }
                        | crate::SymbolTypeFact::Spec { .. }
                )
            ) || matches!(
                ctx.symbol_fact(symbol),
                Some(crate::SymbolTypeFact::Type { constructor })
                    if ctx.interner.store().nominal_identity(TypeId(constructor.0)).is_some()
                        || (ctx.hir.source_standard_types
                            && matches!(ctx.interner.store().get(TypeId(constructor.0)), Some(Type::Enum(_))))
            ) || ctx
                .symbols
                .canonical_symbol(ctx.hir, symbol)
                .and_then(|symbol| ctx.hir.symbols.get(symbol))
                .is_some_and(|symbol| {
                    matches!(symbol.def, SymbolDef::TypeParam { .. })
                        || (matches!(symbol.def, SymbolDef::Item { .. })
                            && matches!(
                                symbol.kind,
                                etas_hir::SymbolKind::Type
                                    | etas_hir::SymbolKind::TypeAlias
                                    | etas_hir::SymbolKind::Enum
                                    | etas_hir::SymbolKind::Agent
                                    | etas_hir::SymbolKind::Spec
                            ))
                })
        }
        _ => false,
    };
    match (standard_name.as_deref().unwrap_or(name), args.as_slice()) {
        // A resolved declaration owns its type identity, including source std
        // declarations whose names also occur in the legacy builtin table.
        _ if resolved_declaration && !legacy_standard_type => {
            lower_declared_path(ctx, path, name, args, span)
        }
        ("Array", [inner]) => ctx.interner.intern(Type::Array(*inner)),
        ("List", [inner]) => ctx.interner.intern(Type::List(*inner)),
        ("Set", [inner]) => ctx.interner.intern(Type::Set(*inner)),
        ("Range", [index]) => ctx.interner.intern(Type::Range { index: *index }),
        ("Slice", [inner]) => ctx.interner.intern(Type::Slice(*inner)),
        ("Option", [inner]) => ctx.interner.intern(Type::Option(*inner)),
        ("Map", [key, value]) => ctx.interner.intern(Type::Map {
            key: *key,
            value: *value,
        }),
        ("Result", [ok, err]) => ctx.interner.intern(Type::Result { ok: *ok, err: *err }),
        ("Trusted", [inner]) => ctx.interner.intern(Type::Trust {
            wrapper: TrustWrapper::Trusted,
            inner: *inner,
        }),
        ("Untrusted", [inner]) => ctx.interner.intern(Type::Trust {
            wrapper: TrustWrapper::Untrusted,
            inner: *inner,
        }),
        ("Secret", [inner]) => ctx.interner.intern(Type::Trust {
            wrapper: TrustWrapper::Secret,
            inner: *inner,
        }),
        ("Public", [inner]) => ctx.interner.intern(Type::Trust {
            wrapper: TrustWrapper::Public,
            inner: *inner,
        }),
        ("Sanitized", [inner]) => ctx.interner.intern(Type::Trust {
            wrapper: TrustWrapper::Sanitized,
            inner: *inner,
        }),
        ("Prompt", []) => ctx.interner.intern(Type::Prompt),
        ("PromptPart", []) => ctx.interner.intern(Type::PromptPart),
        ("Message", [inner]) => {
            let registry = ctx.std_registry.clone();
            super::std::lower_message_type(ctx, &registry, *inner)
        }
        ("MemorySelection", [inner]) => ctx.interner.intern(Type::MemorySelection(*inner)),
        ("MemoryRegion", [schema]) => ctx.interner.intern(Type::MemoryRegion(*schema)),
        ("Store", [key, value]) => ctx.interner.intern(Type::Store {
            key: *key,
            value: *value,
        }),
        _ => lower_declared_path(ctx, path, name, args, span),
    }
}

fn lower_declared_path(
    ctx: &mut TypePipelineContext<'_>,
    path: &etas_hir::ResolvedPath,
    name: &str,
    args: Vec<TypeId>,
    span: etas_core::Span,
) -> TypeId {
    match resolved_source_type_symbol(ctx, path) {
        Some(symbol) => {
            let canonical_symbol = ctx
                .symbols
                .canonical_symbol(ctx.hir, symbol)
                .unwrap_or(symbol);
            if ctx
                .hir
                .symbols
                .get(canonical_symbol)
                .is_some_and(|symbol| matches!(symbol.def, SymbolDef::TypeParam { .. }))
            {
                if let Some(bound) = ctx.bound_type_parameter(canonical_symbol) {
                    return applied_or_constructor(ctx, bound, args);
                }
                let parameter = ctx.interner.intern(Type::Named(NamedTypeRef {
                    name: name.to_owned(),
                    source_parameter: Some(canonical_symbol),
                }));
                return applied_or_constructor(ctx, parameter, args);
            }
            if ctx
                .symbol_fact(symbol)
                .is_some_and(|fact| matches!(fact, crate::SymbolTypeFact::Spec { .. }))
                || ctx
                    .hir
                    .symbols
                    .get(canonical_symbol)
                    .is_some_and(|symbol| symbol.kind == etas_hir::SymbolKind::Spec)
            {
                ctx.diagnostics.push(Diagnostic::type_check(
                    TypeDiagnosticCode::TypeMismatch,
                    span,
                    "spec name cannot be used as a value type",
                ));
                return ctx.interner.primitive(PrimitiveType::Never);
            }
            let fact = ctx.symbol_fact(symbol).cloned();
            if let Some(fact) = fact {
                return match fact {
                    crate::SymbolTypeFact::Agent { signature } => {
                        let params = match ctx.interner.store().get(signature.component) {
                            Some(Type::Nominal(nominal)) => nominal.params.clone(),
                            _ => Vec::new(),
                        };
                        apply_declared_type_args(ctx, signature.component, params, args)
                    }
                    crate::SymbolTypeFact::TypeAlias { target, params } => apply_alias_type(
                        ctx,
                        target,
                        params.into_iter().map(|p| p.name).collect(),
                        args,
                        span,
                    ),
                    crate::SymbolTypeFact::NominalType {
                        constructor,
                        params,
                        ..
                    } => apply_declared_type_args(ctx, TypeId(constructor.0), params, args),
                    crate::SymbolTypeFact::Type { constructor } => {
                        if let Some(symbol) = ctx.hir.symbols.get(canonical_symbol) {
                            if let SymbolDef::Item { item } = symbol.def {
                                if let Some(HirItem::Enum(decl)) = ctx.hir.items.get(item) {
                                    if args.len() != decl.type_params.len() {
                                        return ctx.interner.primitive(PrimitiveType::Never);
                                    }
                                }
                            }
                        }
                        known_or_named_path(ctx, name, TypeId(constructor.0), args, span)
                    }
                    _ => named_or_applied(ctx, name, args),
                };
            }
            if let Some(base_type) = ctx.type_fact_as_type_id(symbol) {
                applied_or_constructor(ctx, base_type, args)
            } else if let Some(alias_type) =
                lower_source_declared_alias(ctx, canonical_symbol, args.clone(), span)
            {
                alias_type
            } else if let Some(base_type) = lower_source_declared_type(ctx, canonical_symbol) {
                applied_or_constructor(ctx, base_type, args)
            } else {
                known_or_named_path(ctx, name, None, args, span)
            }
        }
        None => known_or_named_path(ctx, name, None, args, span),
    }
}

pub(crate) fn agent_context_ref(
    ctx: &TypePipelineContext<'_>,
    path: &etas_hir::ResolvedPath,
) -> Option<crate::AgentContextRef> {
    if let Some(symbol) = resolved_source_type_symbol(ctx, path)
        .and_then(|symbol| ctx.symbols.canonical_symbol(ctx.hir, symbol))
        && (ctx.hir.agent_methods.contains_key(&symbol) || ctx.agent_method(symbol).is_some())
    {
        return Some(symbol.into());
    }
    let ResolveResult::PartiallyResolved(partial) = &path.resolution else {
        return None;
    };
    let [name] = partial.remaining.as_slice() else {
        return None;
    };
    let owner = ctx
        .symbols
        .canonical_symbol(ctx.hir, partial.resolved_prefix?)?;
    if let Some(method) = ctx.hir.agent_method(owner, name) {
        return Some(method.symbol.into());
    }
    let facts = ctx
        .signature_base
        .map_or(&ctx.signature_facts, |base| &base.facts);
    if let Some(SymbolDef::Item { item }) = ctx.hir.symbols.get(owner).map(|symbol| &symbol.def)
        && let Some(crate::ItemSignature::Agent(agent)) = facts.item_signatures.get(item)
        && let Some(method) = facts.agent_method(agent.component, name)
    {
        return Some(method.symbol.into());
    }
    let crate::facts::reflection::ExternalAgentReflectionFact::Available { header, .. } =
        ctx.external_agent_reflection(owner)?
    else {
        return None;
    };
    let methods = crate::select_external_methods(header).ok()??;
    let crate::ExternalMethodSelection::Selected(callable) = methods.get(name)? else {
        return None;
    };
    Some(crate::AgentContextRef::Portable(Box::new(
        callable.as_method()?.clone(),
    )))
}

pub(crate) fn resolved_source_type_symbol(
    ctx: &TypePipelineContext<'_>,
    path: &etas_hir::ResolvedPath,
) -> Option<etas_hir::SymbolId> {
    ctx.symbols.resolved_type_symbol(ctx.hir, path)
}

fn apply_alias_type(
    ctx: &mut TypePipelineContext<'_>,
    target: TypeId,
    params: Vec<String>,
    args: Vec<TypeId>,
    span: etas_core::Span,
) -> TypeId {
    if params.len() != args.len() {
        return ctx.interner.primitive(PrimitiveType::Never);
    }
    if params.is_empty() {
        return target;
    }
    let mut rows = std::collections::HashMap::new();
    let mut substitutions = std::collections::HashMap::new();
    for (name, argument) in params.into_iter().zip(args) {
        if let Some(Type::Handler(HandlerType {
            handled,
            produced: HandlerProducedEffects::Infer,
            result: None,
        })) = ctx.interner.store().get(argument)
        {
            rows.insert(name.clone(), handled.clone());
        }
        substitutions.insert(name, argument);
    }
    match crate::TypeSubstitutionEngine::materializing(&mut ctx.interner, &substitutions, None)
        .with_effect_arguments(&rows)
        .substitute(target)
    {
        Ok(ty) => ty,
        Err(error) => {
            ctx.diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::IncompleteTypeFacts,
                span,
                format!("type alias substitution failed: {error}"),
            ));
            ctx.interner.primitive(PrimitiveType::Never)
        }
    }
}

fn apply_declared_type_args(
    ctx: &mut TypePipelineContext<'_>,
    base: TypeId,
    params: Vec<String>,
    args: Vec<TypeId>,
) -> TypeId {
    if args.is_empty()
        && matches!(ctx.interner.store().get(base), Some(Type::Nominal(declaration)) if declaration.read_only_erased_view)
    {
        return base;
    }
    if params.len() != args.len() {
        // The queued TypeApplication owns arity/bound diagnostics. Do not
        // construct a malformed Applied node or report the same error twice.
        return ctx.interner.primitive(PrimitiveType::Never);
    }
    applied_or_constructor(ctx, base, args)
}

fn lower_source_declared_alias(
    ctx: &mut TypePipelineContext<'_>,
    symbol: etas_hir::SymbolId,
    args: Vec<TypeId>,
    span: etas_core::Span,
) -> Option<TypeId> {
    if ctx.lowering_type_stack.contains(&symbol) {
        let Some(symbol_data) = ctx.hir.symbols.get(symbol) else {
            return Some(ctx.interner.primitive(PrimitiveType::Never));
        };
        ctx.diagnostics.push(Diagnostic::type_check(
            TypeDiagnosticCode::UnknownType,
            symbol_data.definition_span,
            "recursive type alias or representation cannot be lowered",
        ));
        return Some(ctx.interner.primitive(PrimitiveType::Never));
    }
    let symbol_data = ctx.hir.symbols.get(symbol)?;
    let item = match &symbol_data.def {
        SymbolDef::Item { item } => *item,
        _ => symbol_data.defining_item?,
    };
    let item_data = ctx.hir.items.get(item).cloned()?;
    let HirItem::TypeAlias(decl) = item_data else {
        return None;
    };
    ctx.lowering_type_stack.push(symbol);
    let target = lower_type_ref(ctx, decl.target);
    ctx.lowering_type_stack.pop();
    let target = target?;
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
    Some(apply_alias_type(ctx, target, params, args, span))
}

fn lower_source_declared_type(
    ctx: &mut TypePipelineContext<'_>,
    symbol: etas_hir::SymbolId,
) -> Option<TypeId> {
    if ctx.lowering_type_stack.contains(&symbol) {
        let Some(symbol_data) = ctx.hir.symbols.get(symbol) else {
            return Some(ctx.interner.primitive(PrimitiveType::Never));
        };
        ctx.diagnostics.push(Diagnostic::type_check(
            TypeDiagnosticCode::UnknownType,
            symbol_data.definition_span,
            "recursive type alias or representation cannot be lowered",
        ));
        return Some(ctx.interner.primitive(PrimitiveType::Never));
    }
    let symbol_data = ctx.hir.symbols.get(symbol)?;
    let item = match &symbol_data.def {
        SymbolDef::Item { item } => *item,
        _ => symbol_data.defining_item?,
    };
    ctx.lowering_type_stack.push(symbol);
    let Some(item_data) = ctx.hir.items.get(item).cloned() else {
        ctx.lowering_type_stack.pop();
        return None;
    };
    match item_data {
        HirItem::TypeAlias(decl) => {
            let lowered = lower_type_ref(ctx, decl.target);
            ctx.lowering_type_stack.pop();
            lowered
        }
        HirItem::Type(decl) => {
            let name = canonical_source_type_name(ctx, decl.symbol)
                .unwrap_or_else(|| format!("type{}", decl.symbol.0));
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
            let representation = match decl.body {
                HirTypeDeclBody::Bodyless => None,
                HirTypeDeclBody::Representation(ty) => lower_type_ref(ctx, ty),
                HirTypeDeclBody::Sequence {
                    element,
                    tail,
                    span,
                } => lower_sequence_representation(
                    ctx,
                    decl.symbol,
                    &decl.type_params,
                    element,
                    tail,
                    span,
                ),
            };
            let lowered = Some(ctx.interner.intern(Type::Nominal(NominalTypeRef {
                read_only_erased_view: false,
                name,
                params,
                representation,
            })));
            ctx.lowering_type_stack.pop();
            lowered
        }
        HirItem::Enum(decl) => {
            let name = canonical_source_type_name(ctx, decl.symbol)
                .unwrap_or_else(|| format!("enum{}", decl.symbol.0));
            let lowered = Some(ctx.interner.intern(Type::Enum(EnumTypeRef { name })));
            ctx.lowering_type_stack.pop();
            lowered
        }
        _ => {
            ctx.lowering_type_stack.pop();
            None
        }
    }
}

pub(crate) fn canonical_source_type_name(
    ctx: &TypePipelineContext<'_>,
    symbol: etas_hir::SymbolId,
) -> Option<String> {
    let symbol_data = ctx.hir.symbols.get(symbol)?;
    let module = ctx.hir.modules_arena.get(symbol_data.defining_module)?;
    let local_name = symbol_data
        .name
        .rsplit('.')
        .next()
        .unwrap_or(&symbol_data.name)
        .to_owned();
    let Some(module_name) = &module.name else {
        return Some(local_name);
    };
    let mut segments = module_name
        .segments
        .iter()
        .map(|segment| segment.name.clone())
        .collect::<Vec<_>>();
    segments.push(local_name);
    Some(segments.join("."))
}

fn named_or_applied(ctx: &mut TypePipelineContext<'_>, name: &str, args: Vec<TypeId>) -> TypeId {
    let base = ctx.interner.intern(Type::Named(NamedTypeRef {
        source_parameter: None,
        name: name.to_owned(),
    }));
    applied_or_constructor(ctx, base, args)
}

fn known_or_named_path(
    ctx: &mut TypePipelineContext<'_>,
    name: &str,
    resolved_base: impl Into<Option<TypeId>>,
    args: Vec<TypeId>,
    span: etas_core::Span,
) -> TypeId {
    if let Some(expected) = known_type_constructor_arity(name)
        && args.len() != expected
    {
        ctx.diagnostics.push(Diagnostic::type_check(
            TypeDiagnosticCode::ArityMismatch,
            span,
            "type constructor was given the wrong number of type arguments",
        ));
        return ctx.interner.primitive(PrimitiveType::Never);
    }
    if let Some(base) = resolved_base.into() {
        applied_or_constructor(ctx, base, args)
    } else {
        named_or_applied(ctx, name, args)
    }
}

pub(super) fn applied_or_constructor(
    ctx: &mut TypePipelineContext<'_>,
    base: TypeId,
    args: Vec<TypeId>,
) -> TypeId {
    match (ctx.interner.store().get(base).cloned(), args.as_slice()) {
        (Some(Type::Task(_)), [output, row]) => {
            if let Some(Type::Handler(HandlerType {
                handled,
                produced: HandlerProducedEffects::Infer,
                result: None,
            })) = ctx.interner.store().get(*row)
            {
                let effects = handled.clone();
                return ctx.interner.intern(Type::Task(crate::TaskType {
                    output: *output,
                    effects,
                }));
            }
        }
        (Some(Type::Continuation(mut continuation)), [output]) => {
            continuation.action_output = *output;
            return ctx.interner.intern(Type::Continuation(continuation));
        }
        _ => {}
    }
    if args.is_empty() {
        base
    } else {
        ctx.interner.intern(Type::Applied {
            constructor: crate::TypeConstructorId(base.0),
            args,
        })
    }
}

fn lower_arrow_input(ctx: &mut TypePipelineContext<'_>, input: HirTypeId) -> Option<Vec<TypeId>> {
    match ctx.hir.types[input].clone() {
        HirType::Primitive {
            kind: HirPrimitiveType::Unit,
            ..
        } => Some(Vec::new()),
        HirType::Tuple { elems, .. } => elems
            .into_iter()
            .map(|elem| lower_type_ref(ctx, elem))
            .collect::<Option<Vec<_>>>(),
        _ => lower_type_ref(ctx, input).map(|ty| vec![ty]),
    }
}

fn known_type_constructor_arity(name: &str) -> Option<usize> {
    Some(match name {
        "Array" | "List" | "Set" | "Range" | "Slice" | "Option" | "Message" | "MemorySelection"
        | "MemoryRegion" | "Deque" | "Queue" | "Stack" | "OrderedSet" | "Trusted" | "Untrusted"
        | "Secret" | "Public" | "Sanitized" => 1,
        "Map" | "Result" | "Store" => 2,
        "PriorityQueue" | "OrderedMap" => 2,
        "Prompt" | "PromptPart" => 0,
        _ => return None,
    })
}
