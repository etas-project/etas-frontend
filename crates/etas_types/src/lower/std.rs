use etas_std::{
    StdDecl, StdEffectRef, StdPrimitiveType, StdRegistry, StdStaticArg, StdSymbol, StdSymbolKind,
    StdType, TypeDeclKind,
};

use crate::{
    CallableGenericParam, CallableGenericParamKind, CallableSignature, CheckedSpecBound,
    CheckedSpecRef, CheckedStdSpecImplFact, EffectActionArgKind, EffectActionSignature,
    EffectArgRef, EffectRef, EffectRowRef, FieldType, NamedTypeRef, NominalTypeRef, PrimitiveType,
    RecordType, ResourceHandleType, SpecSignature, SymbolTypeFact, TrustWrapper, Type,
    TypeConstructorId, TypeId, pipeline::context::TypePipelineContext,
};

pub fn lower_std_decl(
    ctx: &mut TypePipelineContext<'_>,
    registry: &StdRegistry,
    symbol: etas_hir::SymbolId,
    decl: &StdDecl,
) -> SymbolTypeFact {
    lower_std_decl_with_kind(ctx, registry, symbol, decl, None, None)
}

pub fn lower_std_symbol(
    ctx: &mut TypePipelineContext<'_>,
    registry: &StdRegistry,
    symbol: etas_hir::SymbolId,
    std_symbol: &StdSymbol,
) -> SymbolTypeFact {
    lower_std_decl_with_kind(
        ctx,
        registry,
        symbol,
        &std_symbol.decl,
        Some(std_symbol.kind),
        Some(std_symbol.qualified_path.join(".")),
    )
}

fn lower_std_decl_with_kind(
    ctx: &mut TypePipelineContext<'_>,
    registry: &StdRegistry,
    symbol: etas_hir::SymbolId,
    decl: &StdDecl,
    kind: Option<StdSymbolKind>,
    qualified_name: Option<String>,
) -> SymbolTypeFact {
    let scope = qualified_name
        .as_ref()
        .map(|path| path.split('.').map(str::to_owned).collect::<Vec<_>>())
        .and_then(|mut path| {
            path.pop()?;
            Some(path)
        });
    let scope = scope.as_deref();
    match decl {
        StdDecl::Type(decl) if kind == Some(StdSymbolKind::Constructor) => {
            lower_std_constructor_decl(ctx, registry, decl, qualified_name.as_deref(), scope)
        }
        StdDecl::Type(decl) => match decl.kind {
            TypeDeclKind::Spec => {
                record_std_spec_signature(ctx, symbol, decl, true);
                SymbolTypeFact::Spec { symbol }
            }
            TypeDeclKind::Wrapper => {
                lower_std_constructor_decl(ctx, registry, decl, qualified_name.as_deref(), scope)
            }
            _ => {
                if decl.derivable {
                    record_std_spec_signature(ctx, symbol, decl, false);
                }
                let ty =
                    lower_std_type_decl_with_name(ctx, registry, decl, qualified_name.as_deref());
                match ctx.interner.store().get(ty) {
                    Some(Type::Nominal(nominal)) => SymbolTypeFact::NominalType {
                        constructor: TypeConstructorId(ty.0),
                        params: nominal.params.clone(),
                        representation: nominal.representation,
                    },
                    _ => SymbolTypeFact::Type {
                        constructor: TypeConstructorId(ty.0),
                    },
                }
            }
        },
        StdDecl::Flow(flow) => SymbolTypeFact::Flow {
            signature: CallableSignature {
                generic_params: lower_std_generic_params(ctx, registry, &flow.type_params),
                params: flow
                    .params
                    .iter()
                    .map(|ty| lower_std_type_with_scope(ctx, registry, ty, scope))
                    .collect(),
                output: lower_std_type_with_scope(ctx, registry, &flow.output, scope),
                effects: std_effect_row_with_scope(ctx, registry, &flow.public_effects, scope),
                requested_actions: std_effect_row_with_scope(
                    ctx,
                    registry,
                    &flow.requested_actions,
                    scope,
                ),
            },
        },
        StdDecl::Tool(tool) => SymbolTypeFact::Tool {
            signature: CallableSignature {
                generic_params: Vec::new(),
                params: tool
                    .params
                    .iter()
                    .map(|ty| lower_std_type_with_scope(ctx, registry, ty, scope))
                    .collect(),
                output: lower_std_type_with_scope(ctx, registry, &tool.output, scope),
                effects: std_effect_row_with_scope(ctx, registry, &tool.effects, scope),
                requested_actions: None,
            },
        },
        StdDecl::Effect(_) => SymbolTypeFact::Effect { symbol },
        StdDecl::EffectAction(action) => SymbolTypeFact::EffectAction {
            signature: lower_std_effect_action_signature(ctx, registry, action),
        },
        StdDecl::Value(value) => SymbolTypeFact::Value {
            ty: lower_std_type_with_scope(ctx, registry, &value.ty, scope),
        },
        StdDecl::Requirement(requirement) => SymbolTypeFact::Flow {
            signature: CallableSignature {
                generic_params: Vec::new(),
                params: requirement
                    .params
                    .iter()
                    .map(|ty| lower_std_type_with_scope(ctx, registry, ty, scope))
                    .collect(),
                output: lower_named_std_type_with_scope(ctx, registry, "Limit", scope),
                effects: None,
                requested_actions: None,
            },
        },
    }
}

pub fn lower_std_spec_impls(
    ctx: &mut TypePipelineContext<'_>,
    registry: &StdRegistry,
) -> Vec<CheckedStdSpecImplFact> {
    registry
        .spec_impls()
        .map(|implementation| CheckedStdSpecImplFact {
            self_type: lower_std_type(ctx, registry, &implementation.self_type),
            spec: implementation.spec.path.clone(),
            args: implementation
                .spec
                .args
                .iter()
                .map(|arg| lower_std_type(ctx, registry, arg))
                .collect(),
        })
        .collect()
}

fn lower_std_generic_params(
    ctx: &mut TypePipelineContext<'_>,
    registry: &StdRegistry,
    params: &[etas_std::StdGenericParam],
) -> Vec<CallableGenericParam> {
    params
        .iter()
        .map(|param| CallableGenericParam {
            kind: CallableGenericParamKind::Type,
            subject: ctx.interner.intern(Type::Named(NamedTypeRef {
                name: param.name.clone(),
            })),
            name: param.name.clone(),
            bounds: param
                .bounds
                .iter()
                .map(|bound| CheckedSpecBound {
                    spec: CheckedSpecRef::Std(bound.path.clone()),
                    args: bound
                        .args
                        .iter()
                        .map(|arg| lower_std_type(ctx, registry, arg))
                        .collect(),
                })
                .collect(),
        })
        .collect()
}

pub fn lower_std_effect_action_signature(
    ctx: &mut TypePipelineContext<'_>,
    registry: &StdRegistry,
    action: &etas_std::EffectActionDecl,
) -> EffectActionSignature {
    let effect_args = action
        .effect_args
        .iter()
        .map(lower_std_action_arg_kind)
        .collect::<Vec<_>>();
    let selector_len = effect_args.len();
    EffectActionSignature {
        generic_params: lower_std_generic_params(ctx, registry, &action.type_params),
        params: action
            .params
            .iter()
            .map(|ty| lower_std_type(ctx, registry, ty))
            .collect(),
        output: lower_std_type(ctx, registry, &action.output),
        effect_args,
        selector_param_names: action.selector_param_names.clone(),
        selector_defaults: vec![None; selector_len],
        returns_never: matches!(action.output, StdType::Primitive(StdPrimitiveType::Never)),
    }
}

fn record_std_spec_signature(
    ctx: &mut TypePipelineContext<'_>,
    symbol: etas_hir::SymbolId,
    decl: &etas_std::TypeDecl,
    include_decl_params: bool,
) {
    ctx.signature_facts.spec_signatures.insert(
        symbol,
        SpecSignature {
            symbol,
            name: decl.name.clone(),
            kind: crate::SpecKind::TypeSpec,
            params: Vec::new(),
            param_names: if include_decl_params {
                decl.params.iter().map(|param| param.name.clone()).collect()
            } else {
                Vec::new()
            },
            callable: None,
            methods: Vec::new(),
            super_specs: Vec::new(),
        },
    );
}

fn lower_std_constructor_decl(
    ctx: &mut TypePipelineContext<'_>,
    registry: &StdRegistry,
    decl: &etas_std::TypeDecl,
    qualified_name: Option<&str>,
    scope: Option<&[String]>,
) -> SymbolTypeFact {
    let params = match decl.name.as_str() {
        "Some" => vec![lower_std_type_with_scope(
            ctx,
            registry,
            &StdType::Var("T".to_owned()),
            scope,
        )],
        "Ok" => vec![lower_std_type_with_scope(
            ctx,
            registry,
            &StdType::Var("T".to_owned()),
            scope,
        )],
        "Err" => vec![lower_std_type_with_scope(
            ctx,
            registry,
            &StdType::Var("E".to_owned()),
            scope,
        )],
        "None" => Vec::new(),
        _ => decl
            .params
            .iter()
            .map(|param| {
                lower_std_type_with_scope(ctx, registry, &StdType::Var(param.name.clone()), scope)
            })
            .collect::<Vec<_>>(),
    };
    let output = match decl.name.as_str() {
        "Some" | "None" => {
            let inner =
                lower_std_type_with_scope(ctx, registry, &StdType::Var("T".to_owned()), scope);
            ctx.interner.intern(Type::Option(inner))
        }
        "Ok" | "Err" => {
            let ok = lower_std_type_with_scope(ctx, registry, &StdType::Var("T".to_owned()), scope);
            let err =
                lower_std_type_with_scope(ctx, registry, &StdType::Var("E".to_owned()), scope);
            ctx.interner.intern(Type::Result { ok, err })
        }
        "Trusted" | "Untrusted" | "Secret" | "Public" | "Sanitized" => {
            let inner =
                lower_std_type_with_scope(ctx, registry, &StdType::Var("T".to_owned()), scope);
            ctx.interner.intern(Type::Trust {
                wrapper: trust_wrapper_from_name(&decl.name).expect("checked wrapper name"),
                inner,
            })
        }
        _ => lower_std_type_decl_with_name(ctx, registry, decl, qualified_name),
    };
    SymbolTypeFact::Flow {
        signature: CallableSignature {
            generic_params: lower_std_generic_params(ctx, registry, &decl.params),
            params,
            output,
            effects: None,
            requested_actions: None,
        },
    }
}

pub fn lower_std_type_decl(
    ctx: &mut TypePipelineContext<'_>,
    registry: &StdRegistry,
    decl: &etas_std::TypeDecl,
) -> TypeId {
    lower_std_type_decl_with_name(ctx, registry, decl, None)
}

pub fn lower_std_type_symbol(
    ctx: &mut TypePipelineContext<'_>,
    registry: &StdRegistry,
    symbol: &StdSymbol,
) -> Option<TypeId> {
    let StdDecl::Type(decl) = &symbol.decl else {
        return None;
    };
    Some(lower_std_type_decl_with_name(
        ctx,
        registry,
        decl,
        Some(&symbol.qualified_path.join(".")),
    ))
}

fn lower_std_type_decl_with_name(
    ctx: &mut TypePipelineContext<'_>,
    registry: &StdRegistry,
    decl: &etas_std::TypeDecl,
    qualified_name: Option<&str>,
) -> TypeId {
    let scope = qualified_name.and_then(qualified_scope);
    lower_std_type_constructor(
        ctx,
        registry,
        qualified_name.unwrap_or(&decl.name),
        &decl.params,
        decl.kind,
        decl.representation.as_ref(),
        scope.as_deref(),
    )
}

pub fn lower_std_type(
    ctx: &mut TypePipelineContext<'_>,
    registry: &StdRegistry,
    ty: &StdType,
) -> TypeId {
    lower_std_type_with_scope(ctx, registry, ty, None)
}

fn lower_std_type_with_scope(
    ctx: &mut TypePipelineContext<'_>,
    registry: &StdRegistry,
    ty: &StdType,
    scope: Option<&[String]>,
) -> TypeId {
    match ty {
        StdType::Primitive(primitive) => ctx.interner.primitive(match primitive {
            StdPrimitiveType::Bool => PrimitiveType::Bool,
            StdPrimitiveType::I8 => PrimitiveType::I8,
            StdPrimitiveType::I16 => PrimitiveType::I16,
            StdPrimitiveType::I32 => PrimitiveType::I32,
            StdPrimitiveType::I64 => PrimitiveType::I64,
            StdPrimitiveType::I128 => PrimitiveType::I128,
            StdPrimitiveType::ISize => PrimitiveType::ISize,
            StdPrimitiveType::U8 => PrimitiveType::U8,
            StdPrimitiveType::U16 => PrimitiveType::U16,
            StdPrimitiveType::U32 => PrimitiveType::U32,
            StdPrimitiveType::U64 => PrimitiveType::U64,
            StdPrimitiveType::U128 => PrimitiveType::U128,
            StdPrimitiveType::USize => PrimitiveType::USize,
            StdPrimitiveType::F32 => PrimitiveType::F32,
            StdPrimitiveType::F64 => PrimitiveType::F64,
            StdPrimitiveType::Char => PrimitiveType::Char,
            StdPrimitiveType::String => PrimitiveType::String,
            StdPrimitiveType::Bytes => PrimitiveType::Bytes,
            StdPrimitiveType::Unit => PrimitiveType::Unit,
            StdPrimitiveType::Never => PrimitiveType::Never,
        }),
        StdType::Var(name) => ctx
            .interner
            .intern(Type::Named(NamedTypeRef { name: name.clone() })),
        StdType::Support(kind) => ctx.interner.intern(Type::Named(NamedTypeRef {
            name: kind.source_name().to_owned(),
        })),
        StdType::Array(inner) => {
            let inner = lower_std_type_with_scope(ctx, registry, inner, scope);
            ctx.interner.intern(Type::Array(inner))
        }
        StdType::List(inner) => {
            let inner = lower_std_type_with_scope(ctx, registry, inner, scope);
            ctx.interner.intern(Type::List(inner))
        }
        StdType::Map { key, value } => {
            let key = lower_std_type_with_scope(ctx, registry, key, scope);
            let value = lower_std_type_with_scope(ctx, registry, value, scope);
            ctx.interner.intern(Type::Map { key, value })
        }
        StdType::Set(inner) => {
            let inner = lower_std_type_with_scope(ctx, registry, inner, scope);
            ctx.interner.intern(Type::Set(inner))
        }
        StdType::Range(inner) => {
            let index = lower_std_type_with_scope(ctx, registry, inner, scope);
            ctx.interner.intern(Type::Range { index })
        }
        StdType::Slice(inner) => {
            let inner = lower_std_type_with_scope(ctx, registry, inner, scope);
            ctx.interner.intern(Type::Slice(inner))
        }
        StdType::Option(inner) => {
            let inner = lower_std_type_with_scope(ctx, registry, inner, scope);
            ctx.interner.intern(Type::Option(inner))
        }
        StdType::Result { ok, err } => {
            let ok = lower_std_type_with_scope(ctx, registry, ok, scope);
            let err = lower_std_type_with_scope(ctx, registry, err, scope);
            ctx.interner.intern(Type::Result { ok, err })
        }
        StdType::Tuple(elements) => {
            let elements = elements
                .iter()
                .map(|element| lower_std_type_with_scope(ctx, registry, element, scope))
                .collect();
            ctx.interner.intern(Type::Tuple(elements))
        }
        StdType::Schema(inner) => {
            let inner = lower_std_type_with_scope(ctx, registry, inner, scope);
            ctx.interner.intern(Type::Schema(inner))
        }
        StdType::Trust { wrapper, inner } => {
            let wrapper = lower_trust_wrapper(*wrapper);
            let inner = lower_std_type_with_scope(ctx, registry, inner, scope);
            ctx.interner.intern(Type::Trust { wrapper, inner })
        }
        StdType::Prompt => ctx.interner.intern(Type::Prompt),
        StdType::PromptPart => ctx.interner.intern(Type::PromptPart),
        StdType::Message(inner) => {
            let inner = lower_std_type_with_scope(ctx, registry, inner, scope);
            ctx.interner.intern(Type::Message(inner))
        }
        StdType::MemorySelection(inner) => {
            let inner = lower_std_type_with_scope(ctx, registry, inner, scope);
            ctx.interner.intern(Type::MemorySelection(inner))
        }
        StdType::Store { key, value } => {
            let key = lower_std_type_with_scope(ctx, registry, key, scope);
            let value = lower_std_type_with_scope(ctx, registry, value, scope);
            ctx.interner.intern(Type::Store { key, value })
        }
        StdType::MemoryRegion(inner) => {
            let inner = lower_std_type_with_scope(ctx, registry, inner, scope);
            ctx.interner.intern(Type::MemoryRegion(inner))
        }
        StdType::ResourceHandleMemoryRegion(inner) => {
            let schema = lower_std_type_with_scope(ctx, registry, inner, scope);
            ctx.interner
                .intern(Type::ResourceHandle(ResourceHandleType::MemoryRegion {
                    schema,
                }))
        }
        StdType::Named(name) => lower_named_std_type_with_scope(ctx, registry, name, scope),
        StdType::NamedApplied { name, args } => {
            let constructor = lower_named_std_type_with_scope(ctx, registry, name, scope);
            let args = args
                .iter()
                .map(|arg| lower_std_type_with_scope(ctx, registry, arg, scope))
                .collect();
            ctx.interner.intern(Type::Applied {
                constructor: TypeConstructorId(constructor.0),
                args,
            })
        }
        StdType::Record(fields) => {
            let fields = fields
                .iter()
                .map(|field| FieldType {
                    name: field.name.clone(),
                    ty: lower_std_type_with_scope(ctx, registry, &field.ty, scope),
                })
                .collect();
            ctx.interner.intern(Type::Record(RecordType { fields }))
        }
    }
}

pub fn std_effect_row(
    ctx: &mut TypePipelineContext<'_>,
    registry: &StdRegistry,
    effects: &[StdEffectRef],
) -> Option<EffectRowRef> {
    std_effect_row_with_scope(ctx, registry, effects, None)
}

fn std_effect_row_with_scope(
    ctx: &mut TypePipelineContext<'_>,
    registry: &StdRegistry,
    effects: &[StdEffectRef],
    scope: Option<&[String]>,
) -> Option<EffectRowRef> {
    (!effects.is_empty()).then(|| EffectRowRef {
        effects: effects
            .iter()
            .map(|effect| lower_std_effect_ref(ctx, registry, effect, scope))
            .collect(),
        tail: None,
    })
}

pub fn lower_std_action_arg_kind(kind: &etas_std::EffectActionArgKind) -> EffectActionArgKind {
    match kind {
        etas_std::EffectActionArgKind::Type => EffectActionArgKind::Type,
        etas_std::EffectActionArgKind::MemoryPlace => EffectActionArgKind::MemoryPlace,
        etas_std::EffectActionArgKind::StaticResourcePath { ty } => {
            EffectActionArgKind::StaticResourcePath {
                ty: (*ty).to_owned(),
            }
        }
        etas_std::EffectActionArgKind::StringPattern => EffectActionArgKind::StringPattern,
    }
}

fn lower_named_std_type_with_scope(
    ctx: &mut TypePipelineContext<'_>,
    registry: &StdRegistry,
    name: &str,
    scope: Option<&[String]>,
) -> TypeId {
    match name {
        "bool" => return ctx.interner.primitive(PrimitiveType::Bool),
        "string" => return ctx.interner.primitive(PrimitiveType::String),
        "bytes" => return ctx.interner.primitive(PrimitiveType::Bytes),
        "unit" => return ctx.interner.primitive(PrimitiveType::Unit),
        "never" => return ctx.interner.primitive(PrimitiveType::Never),
        _ => {}
    }

    if !name.contains('.')
        && let Some(scope) = scope
        && let Some((identity, decl)) = lookup_scoped_std_type_decl(registry, scope, name)
    {
        return lower_std_type_constructor(
            ctx,
            registry,
            &identity,
            &decl.params,
            decl.kind,
            decl.representation.as_ref(),
            Some(scope),
        );
    }

    if let Some((identity, decl)) = lookup_std_type_decl(registry, name) {
        let constructor_scope = qualified_scope(&identity);
        return lower_std_type_constructor(
            ctx,
            registry,
            &identity,
            &decl.params,
            decl.kind,
            decl.representation.as_ref(),
            constructor_scope.as_deref(),
        );
    }

    ctx.interner.intern(Type::Named(NamedTypeRef {
        name: name.to_owned(),
    }))
}

fn lookup_scoped_std_type_decl<'a>(
    registry: &'a StdRegistry,
    scope: &[String],
    name: &str,
) -> Option<(String, &'a etas_std::TypeDecl)> {
    let mut path = scope.to_vec();
    path.push(name.to_owned());
    let symbol = registry.lookup_qualified(&path)?;
    match &symbol.decl {
        StdDecl::Type(decl) => Some((path.join("."), decl)),
        _ => None,
    }
}

fn lookup_std_type_decl<'a>(
    registry: &'a StdRegistry,
    name: &str,
) -> Option<(String, &'a etas_std::TypeDecl)> {
    let (identity, symbol) = if name.contains('.') {
        let path = name.split('.').collect::<Vec<_>>();
        (name.to_owned(), registry.lookup_qualified(&path)?)
    } else {
        let symbol = registry
            .lookup_prelude(name)
            .and_then(|symbol| registry.symbol(symbol.id))
            .or_else(|| {
                let mut matches = registry.symbols().filter(|symbol| symbol.name == name);
                let first = matches.next()?;
                matches.next().is_none().then_some(first)
            })?;
        let mut path = registry
            .module(symbol.module)
            .map_or_else(Vec::new, |module| module.path.clone());
        path.push(symbol.name.clone());
        let identity = path.join(".");
        (identity, symbol)
    };
    match &symbol.decl {
        StdDecl::Type(decl) => Some((identity, decl)),
        _ => None,
    }
}

fn lower_std_type_constructor(
    ctx: &mut TypePipelineContext<'_>,
    registry: &StdRegistry,
    name: &str,
    params: &[etas_std::StdGenericParam],
    kind: TypeDeclKind,
    representation: Option<&StdType>,
    scope: Option<&[String]>,
) -> TypeId {
    if let Some(representation) = representation {
        let representation = lower_std_type_with_scope(ctx, registry, representation, scope);
        return ctx.interner.intern(Type::Nominal(NominalTypeRef {
            name: name.to_owned(),
            params: params.iter().map(|param| param.name.clone()).collect(),
            representation: Some(representation),
        }));
    }

    match kind {
        TypeDeclKind::Enum => ctx.interner.intern(Type::Enum(crate::EnumTypeRef {
            name: name.to_owned(),
        })),
        TypeDeclKind::Struct | TypeDeclKind::Wrapper => {
            ctx.interner.intern(Type::Nominal(NominalTypeRef {
                name: name.to_owned(),
                params: params.iter().map(|param| param.name.clone()).collect(),
                representation: None,
            }))
        }
        TypeDeclKind::Primitive | TypeDeclKind::Support | TypeDeclKind::Spec => {
            ctx.interner.intern(Type::Named(NamedTypeRef {
                name: name.to_owned(),
            }))
        }
    }
}

fn qualified_scope(name: &str) -> Option<Vec<String>> {
    let mut path = name.split('.').map(str::to_owned).collect::<Vec<_>>();
    path.pop()?;
    (!path.is_empty()).then_some(path)
}

fn lower_trust_wrapper(wrapper: etas_std::StdTrustWrapper) -> TrustWrapper {
    match wrapper {
        etas_std::StdTrustWrapper::Trusted => TrustWrapper::Trusted,
        etas_std::StdTrustWrapper::Untrusted => TrustWrapper::Untrusted,
        etas_std::StdTrustWrapper::Secret => TrustWrapper::Secret,
        etas_std::StdTrustWrapper::Public => TrustWrapper::Public,
        etas_std::StdTrustWrapper::Sanitized => TrustWrapper::Sanitized,
    }
}

fn trust_wrapper_from_name(name: &str) -> Option<TrustWrapper> {
    Some(match name {
        "Trusted" => TrustWrapper::Trusted,
        "Untrusted" => TrustWrapper::Untrusted,
        "Secret" => TrustWrapper::Secret,
        "Public" => TrustWrapper::Public,
        "Sanitized" => TrustWrapper::Sanitized,
        _ => return None,
    })
}

fn lower_std_effect_ref(
    ctx: &mut TypePipelineContext<'_>,
    registry: &StdRegistry,
    effect: &StdEffectRef,
    scope: Option<&[String]>,
) -> EffectRef {
    EffectRef {
        name: effect.path.join("."),
        args: effect
            .args
            .iter()
            .map(|arg| lower_std_static_arg(ctx, registry, arg, scope))
            .collect(),
    }
}

fn lower_std_static_arg(
    ctx: &mut TypePipelineContext<'_>,
    registry: &StdRegistry,
    arg: &StdStaticArg,
    scope: Option<&[String]>,
) -> EffectArgRef {
    match arg {
        StdStaticArg::Type(ty) => {
            EffectArgRef::Type(lower_std_type_with_scope(ctx, registry, ty, scope))
        }
        StdStaticArg::Path(path) => EffectArgRef::Path(path.clone()),
        StdStaticArg::String(value) => EffectArgRef::String(value.clone()),
        StdStaticArg::Int(value) => EffectArgRef::Int(value.clone()),
        StdStaticArg::Wildcard => EffectArgRef::Wildcard,
    }
}
