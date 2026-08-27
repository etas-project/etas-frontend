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
    let is_path_type = matches!(ctx.hir.types[ty], HirType::Path { .. });
    if !is_path_type && let Some(existing) = ctx.signature_facts.type_refs.get(&ty).copied() {
        return Some(existing);
    }
    let lowered = match ctx.hir.types[ty].clone() {
        HirType::Primitive { kind, .. } => ctx.interner.primitive(map_primitive(kind)),
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
        .filter_map(|arg| lower_type_ref(ctx, *arg))
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
    match (name, args.as_slice()) {
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
        ("Message", [inner]) => ctx.interner.intern(Type::Message(*inner)),
        ("MemorySelection", [inner]) => ctx.interner.intern(Type::MemorySelection(*inner)),
        ("MemoryRegion", [schema]) => ctx.interner.intern(Type::MemoryRegion(*schema)),
        ("Store", [key, value]) => ctx.interner.intern(Type::Store {
            key: *key,
            value: *value,
        }),
        _ => match path.resolution {
            ResolveResult::Resolved(symbol) => {
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
                    return named_or_applied(ctx, name, args);
                }
                if ctx
                    .symbols
                    .symbol_fact(ctx.hir, &ctx.signature_facts, symbol)
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
                let fact = ctx
                    .symbols
                    .symbol_fact(ctx.hir, &ctx.signature_facts, symbol)
                    .cloned();
                if let Some(fact) = fact {
                    return match fact {
                        crate::SymbolTypeFact::TypeAlias { target, params } => {
                            apply_alias_type(ctx, target, params, args, span)
                        }
                        crate::SymbolTypeFact::NominalType {
                            constructor,
                            params,
                            ..
                        } => apply_declared_type_args(
                            ctx,
                            TypeId(constructor.0),
                            params,
                            args,
                            span,
                            "nominal type was given the wrong number of type arguments",
                        ),
                        crate::SymbolTypeFact::Type { constructor } => {
                            known_or_named_path(ctx, name, TypeId(constructor.0), args, span)
                        }
                        _ => named_or_applied(ctx, name, args),
                    };
                }
                if let Some(base_type) =
                    ctx.symbols
                        .type_fact_as_type_id(ctx.hir, &ctx.signature_facts, symbol)
                {
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
            _ => known_or_named_path(ctx, name, None, args, span),
        },
    }
}

fn apply_alias_type(
    ctx: &mut TypePipelineContext<'_>,
    target: TypeId,
    params: Vec<String>,
    args: Vec<TypeId>,
    span: etas_core::Span,
) -> TypeId {
    if params.len() != args.len() {
        ctx.diagnostics.push(Diagnostic::type_check(
            TypeDiagnosticCode::ArityMismatch,
            span,
            "type alias was given the wrong number of type arguments",
        ));
        return ctx.interner.primitive(PrimitiveType::Never);
    }
    if params.is_empty() {
        return target;
    }
    let substitutions = params.into_iter().zip(args).collect();
    match crate::substitute_named_params(&mut ctx.interner, target, &substitutions) {
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
    span: etas_core::Span,
    message: &'static str,
) -> TypeId {
    if params.len() != args.len() {
        ctx.diagnostics.push(Diagnostic::type_check(
            TypeDiagnosticCode::ArityMismatch,
            span,
            message,
        ));
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
    let item = match ctx.hir.symbols.get(symbol) {
        Some(symbol_data) => match &symbol_data.def {
            SymbolDef::Item { item } => *item,
            _ => symbol_data.defining_item?,
        },
        None => return None,
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
    let item = match ctx.hir.symbols.get(symbol) {
        Some(symbol_data) => match &symbol_data.def {
            SymbolDef::Item { item } => *item,
            _ => symbol_data.defining_item?,
        },
        None => return None,
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
            };
            let lowered = Some(ctx.interner.intern(Type::Nominal(NominalTypeRef {
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

fn applied_or_constructor(
    ctx: &mut TypePipelineContext<'_>,
    base: TypeId,
    args: Vec<TypeId>,
) -> TypeId {
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

fn map_primitive(kind: HirPrimitiveType) -> PrimitiveType {
    match kind {
        HirPrimitiveType::Bool => PrimitiveType::Bool,
        HirPrimitiveType::I8 => PrimitiveType::I8,
        HirPrimitiveType::I16 => PrimitiveType::I16,
        HirPrimitiveType::I32 => PrimitiveType::I32,
        HirPrimitiveType::I64 => PrimitiveType::I64,
        HirPrimitiveType::I128 => PrimitiveType::I128,
        HirPrimitiveType::Isize => PrimitiveType::ISize,
        HirPrimitiveType::U8 => PrimitiveType::U8,
        HirPrimitiveType::U16 => PrimitiveType::U16,
        HirPrimitiveType::U32 => PrimitiveType::U32,
        HirPrimitiveType::U64 => PrimitiveType::U64,
        HirPrimitiveType::U128 => PrimitiveType::U128,
        HirPrimitiveType::Usize => PrimitiveType::USize,
        HirPrimitiveType::F32 => PrimitiveType::F32,
        HirPrimitiveType::F64 => PrimitiveType::F64,
        HirPrimitiveType::Char => PrimitiveType::Char,
        HirPrimitiveType::String => PrimitiveType::String,
        HirPrimitiveType::Bytes => PrimitiveType::Bytes,
        HirPrimitiveType::Unit => PrimitiveType::Unit,
        HirPrimitiveType::Never => PrimitiveType::Never,
    }
}
