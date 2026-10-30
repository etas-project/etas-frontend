use etas_core::{Diagnostic, Span, TypeDiagnosticCode};

mod spec_ref;
pub(crate) use spec_ref::resolve_external_spec;

mod package_type;
pub use package_type::{package_effect_arg_input, package_effect_row_input, package_type_input};

use crate::{
    EffectActionArgKind, EffectActionSignature, EffectArgRef, EffectRef, EffectRowRef, FieldType,
    FlowType, HandlerProducedEffects, HandlerType, NamedTypeRef, NominalTypeRef, PrimitiveType,
    RecordType, ResourceHandleType, TrustWrapper, Type, TypeConstructorId, TypeId, TypeOutput,
    TypeStore, pipeline::context::TypePipelineContext,
};

pub fn lower_external_action_signature(
    ctx: &mut TypePipelineContext<'_>,
    span: Span,
    package: crate::ExternalPackageKey,
    action: &crate::ExternalActionSignatureInput,
) -> Option<EffectActionSignature> {
    validate_external_action_selector_metadata(ctx, span, action);
    let effect_args = action
        .effect_args
        .iter()
        .map(|kind| match kind {
            crate::ExternalActionArgKindInput::Type => EffectActionArgKind::Type,
            crate::ExternalActionArgKindInput::MemoryPlace => EffectActionArgKind::MemoryPlace,
            crate::ExternalActionArgKindInput::StaticResourcePath { ty } => {
                EffectActionArgKind::StaticResourcePath { ty: ty.clone() }
            }
            crate::ExternalActionArgKindInput::StringPattern => EffectActionArgKind::StringPattern,
        })
        .collect();
    let generic_params = lower_external_callable_generic_params(
        ctx,
        span,
        package,
        &action.path,
        &action.generic_params,
    )?;
    Some(EffectActionSignature {
        generic_params,
        params: action
            .params
            .iter()
            .map(|ty| lower_external_type(ctx, span, ty))
            .collect(),
        output: lower_external_type(ctx, span, &action.output),
        effect_args,
        selector_param_names: action.selector_param_names.clone(),
        selector_defaults: action
            .selector_defaults
            .iter()
            .map(|arg| {
                arg.as_ref()
                    .map(|arg| lower_external_effect_arg(ctx, span, arg))
            })
            .collect(),
        returns_never: action.returns_never,
        default_handler_effects: None,
    })
}

pub fn lower_external_callable_generic_params(
    ctx: &mut TypePipelineContext<'_>,
    span: Span,
    package: crate::ExternalPackageKey,
    callable_path: &[String],
    params: &[crate::ExternalCallableGenericParamInput],
) -> Option<Vec<crate::CallableGenericParam>> {
    let unique_names = params
        .iter()
        .map(|param| param.name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if unique_names.len() != params.len() {
        invalid_external_metadata(
            ctx,
            span,
            format!(
                "invalid external package metadata: callable `{}` contains duplicate generic parameter names",
                callable_path.join(".")
            ),
        );
        return None;
    }
    let mut generic_params = Vec::with_capacity(params.len());
    for param in params {
        let mut bounds = Vec::with_capacity(param.bounds.len());
        for bound in &param.bounds {
            let spec = match resolve_external_spec(ctx, package, &bound.spec, bound.args.len()) {
                Ok(spec) => spec,
                Err(reason) => {
                    invalid_external_metadata(
                        ctx,
                        span,
                        format!(
                            "invalid external package metadata: callable `{}` generic `{}` references spec `{}`: {reason}",
                            callable_path.join("."),
                            param.name,
                            bound.spec.join(".")
                        ),
                    );
                    return None;
                }
            };
            bounds.push(crate::CheckedSpecBound {
                spec,
                args: bound
                    .args
                    .iter()
                    .map(|arg| lower_external_type(ctx, span, arg))
                    .collect(),
            });
        }
        generic_params.push(crate::CallableGenericParam {
            kind: match param.kind {
                crate::ExternalCallableGenericParamKindInput::Type => {
                    crate::CallableGenericParamKind::Type
                }
                crate::ExternalCallableGenericParamKindInput::Effect => {
                    crate::CallableGenericParamKind::Effect
                }
            },
            name: param.name.clone(),
            subject: ctx.interner.intern(Type::Named(NamedTypeRef {
                source_parameter: None,
                name: param.name.clone(),
            })),
            bounds,
        });
    }
    Some(generic_params)
}

fn validate_external_action_selector_metadata(
    ctx: &mut TypePipelineContext<'_>,
    span: Span,
    action: &crate::ExternalActionSignatureInput,
) {
    if action.selector_param_names.len() != action.effect_args.len() {
        invalid_external_metadata(
            ctx,
            span,
            format!(
                "invalid external package metadata: action `{}` selector_param_names length {} does not match effect_args length {}",
                action.path.join("."),
                action.selector_param_names.len(),
                action.effect_args.len()
            ),
        );
    }
    if action.selector_defaults.len() != action.effect_args.len() {
        invalid_external_metadata(
            ctx,
            span,
            format!(
                "invalid external package metadata: action `{}` selector_defaults length {} does not match effect_args length {}",
                action.path.join("."),
                action.selector_defaults.len(),
                action.effect_args.len()
            ),
        );
    }
    let generic_names = action
        .generic_params
        .iter()
        .map(|param| param.name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if generic_names.len() != action.generic_params.len() {
        invalid_external_metadata(
            ctx,
            span,
            format!(
                "invalid external package metadata: action `{}` contains duplicate generic parameter names",
                action.path.join(".")
            ),
        );
    }
    for (kind, name) in action.effect_args.iter().zip(&action.selector_param_names) {
        if matches!(kind, crate::ExternalActionArgKindInput::Type)
            && (name.is_empty() || !generic_names.contains(name.as_str()))
        {
            invalid_external_metadata(
                ctx,
                span,
                format!(
                    "invalid external package metadata: action `{}` type selector `{name}` does not name a declared generic parameter",
                    action.path.join(".")
                ),
            );
        }
    }
    for (index, (kind, default)) in action
        .effect_args
        .iter()
        .zip(&action.selector_defaults)
        .enumerate()
    {
        let Some(default) = default else {
            continue;
        };
        if !external_effect_arg_matches_action_arg_kind(default, kind) {
            invalid_external_metadata(
                ctx,
                span,
                format!(
                    "invalid external package metadata: action `{}` selector default at index {index} does not match selector kind",
                    action.path.join(".")
                ),
            );
        }
    }
}

fn external_effect_arg_matches_action_arg_kind(
    arg: &crate::ExternalEffectArgInput,
    kind: &crate::ExternalActionArgKindInput,
) -> bool {
    if matches!(arg, crate::ExternalEffectArgInput::Wildcard) {
        return true;
    }
    match kind {
        crate::ExternalActionArgKindInput::Type => {
            matches!(arg, crate::ExternalEffectArgInput::Type(_))
        }
        crate::ExternalActionArgKindInput::MemoryPlace => {
            matches!(arg, crate::ExternalEffectArgInput::Path(_))
        }
        crate::ExternalActionArgKindInput::StaticResourcePath { .. } => {
            matches!(arg, crate::ExternalEffectArgInput::Path(_))
        }
        crate::ExternalActionArgKindInput::StringPattern => {
            matches!(
                arg,
                crate::ExternalEffectArgInput::String(_)
                    | crate::ExternalEffectArgInput::Int(_)
                    | crate::ExternalEffectArgInput::Path(_)
            )
        }
    }
}

pub fn lower_external_type(
    ctx: &mut TypePipelineContext<'_>,
    span: Span,
    ty: &crate::ExternalTypeInput,
) -> TypeId {
    match ty {
        crate::ExternalTypeInput::Primitive(name) => primitive_from_name(ctx, span, name),
        crate::ExternalTypeInput::Var(name) => ctx.interner.intern(Type::Named(NamedTypeRef {
            source_parameter: None,
            name: name.clone(),
        })),
        crate::ExternalTypeInput::Enum { path, .. } => {
            let ty = ctx.interner.intern(Type::Enum(crate::EnumTypeRef {
                name: path.join("."),
            }));
            ctx.external_type_paths.insert(path.clone(), ty);
            ty
        }
        crate::ExternalTypeInput::Named(path) => {
            let Some(ty) = checked_external_or_std_type(ctx, path) else {
                invalid_external_metadata(
                    ctx,
                    span,
                    format!(
                        "invalid external package metadata: named type `{}` has no checked package binding",
                        path.join(".")
                    ),
                );
                return ctx.interner.primitive(PrimitiveType::Never);
            };
            ty
        }
        crate::ExternalTypeInput::Applied { path, args } => {
            let Some(constructor) = checked_external_or_std_type(ctx, path) else {
                invalid_external_metadata(
                    ctx,
                    span,
                    format!(
                        "invalid external package metadata: applied type `{}` has no checked package binding",
                        path.join(".")
                    ),
                );
                return ctx.interner.primitive(PrimitiveType::Never);
            };
            let args = args
                .iter()
                .map(|arg| lower_external_type(ctx, span, arg))
                .collect();
            ctx.interner.intern(Type::Applied {
                constructor: TypeConstructorId(constructor.0),
                args,
            })
        }
        crate::ExternalTypeInput::Alias { target, .. } => lower_external_type(ctx, span, target),
        crate::ExternalTypeInput::Nominal {
            path,
            representation,
        } => {
            if let Some(identity) = checked_external_or_std_type(ctx, path) {
                return identity;
            }
            let identity = lower_external_type_declaration(ctx, span, path, &[], None);
            let representation = representation
                .as_ref()
                .map(|representation| lower_external_type(ctx, span, representation));
            ctx.interner.define_nominal(identity, representation);
            identity
        }
        crate::ExternalTypeInput::Array(inner) => {
            let inner = lower_external_type(ctx, span, inner);
            ctx.interner.intern(Type::Array(inner))
        }
        crate::ExternalTypeInput::List(inner) => {
            let inner = lower_external_type(ctx, span, inner);
            ctx.interner.intern(Type::List(inner))
        }
        crate::ExternalTypeInput::Map { key, value } => {
            let key = lower_external_type(ctx, span, key);
            let value = lower_external_type(ctx, span, value);
            ctx.interner.intern(Type::Map { key, value })
        }
        crate::ExternalTypeInput::Set(inner) => {
            let inner = lower_external_type(ctx, span, inner);
            ctx.interner.intern(Type::Set(inner))
        }
        crate::ExternalTypeInput::Range(inner) => {
            let index = lower_external_type(ctx, span, inner);
            ctx.interner.intern(Type::Range { index })
        }
        crate::ExternalTypeInput::Slice(inner) => {
            let inner = lower_external_type(ctx, span, inner);
            ctx.interner.intern(Type::Slice(inner))
        }
        crate::ExternalTypeInput::Option(inner) => {
            let inner = lower_external_type(ctx, span, inner);
            ctx.interner.intern(Type::Option(inner))
        }
        crate::ExternalTypeInput::Result { ok, err } => {
            let ok = lower_external_type(ctx, span, ok);
            let err = lower_external_type(ctx, span, err);
            ctx.interner.intern(Type::Result { ok, err })
        }
        crate::ExternalTypeInput::Record { fields } => {
            let fields = fields
                .iter()
                .map(|field| FieldType {
                    name: field.name.clone(),
                    ty: lower_external_type(ctx, span, &field.ty),
                })
                .collect();
            ctx.interner.intern(Type::Record(RecordType { fields }))
        }
        crate::ExternalTypeInput::Tuple(elements) => {
            let elements = elements
                .iter()
                .map(|ty| lower_external_type(ctx, span, ty))
                .collect();
            ctx.interner.intern(Type::Tuple(elements))
        }
        crate::ExternalTypeInput::Function {
            input,
            output,
            effects,
        } => {
            let input = input
                .iter()
                .map(|ty| lower_external_type(ctx, span, ty))
                .collect();
            let output = lower_external_type(ctx, span, output);
            let effects = effects
                .as_ref()
                .map(|row| lower_external_effect_row(ctx, span, row));
            ctx.interner.intern(Type::Function(FlowType {
                input,
                output,
                effects,
            }))
        }
        crate::ExternalTypeInput::Handler {
            handled,
            produced,
            result,
        } => {
            let result = result
                .as_ref()
                .map(|result| lower_external_type(ctx, span, result));
            let handled = lower_external_effect_row(ctx, span, handled);
            let produced = produced
                .as_ref()
                .map(|row| lower_external_effect_row(ctx, span, row))
                .map(HandlerProducedEffects::Explicit)
                .unwrap_or(HandlerProducedEffects::Infer);
            ctx.interner.intern(Type::Handler(HandlerType {
                handled,
                produced,
                result,
            }))
        }
        crate::ExternalTypeInput::Trust { wrapper, inner } => {
            let inner = lower_external_type(ctx, span, inner);
            let Some(wrapper) = external_trust_wrapper(wrapper) else {
                invalid_external_metadata(
                    ctx,
                    span,
                    format!("invalid external package metadata: unknown trust wrapper `{wrapper}`"),
                );
                return ctx.interner.primitive(PrimitiveType::Never);
            };
            ctx.interner.intern(Type::Trust { wrapper, inner })
        }
        crate::ExternalTypeInput::Prompt => ctx.interner.intern(Type::Prompt),
        crate::ExternalTypeInput::PromptPart => ctx.interner.intern(Type::PromptPart),
        crate::ExternalTypeInput::Message(inner) => {
            let inner = lower_external_type(ctx, span, inner);
            ctx.interner.intern(Type::Message(inner))
        }
        crate::ExternalTypeInput::MemorySelection(inner) => {
            let inner = lower_external_type(ctx, span, inner);
            ctx.interner.intern(Type::MemorySelection(inner))
        }
        crate::ExternalTypeInput::Store { key, value } => {
            let key = lower_external_type(ctx, span, key);
            let value = lower_external_type(ctx, span, value);
            ctx.interner.intern(Type::Store { key, value })
        }
        crate::ExternalTypeInput::MemoryRegion(inner) => {
            let inner = lower_external_type(ctx, span, inner);
            ctx.interner.intern(Type::MemoryRegion(inner))
        }
        crate::ExternalTypeInput::ResourceHandle { name, args } => {
            let args = args
                .iter()
                .map(|ty| lower_external_type(ctx, span, ty))
                .collect();
            ctx.interner
                .intern(Type::ResourceHandle(ResourceHandleType::Other {
                    name: name.clone(),
                    args,
                }))
        }
    }
}

fn checked_external_or_std_type(
    ctx: &mut TypePipelineContext<'_>,
    path: &[String],
) -> Option<TypeId> {
    if let Some(ty) = ctx.external_type_paths.get(path).copied() {
        return Some(ty);
    }
    let registry = ctx.std_registry.clone();
    let symbol = registry.lookup_qualified(path)?;
    crate::lower::std::lower_std_type_symbol(ctx, &registry, symbol)
}

pub fn lower_external_type_declaration(
    ctx: &mut TypePipelineContext<'_>,
    span: Span,
    path: &[String],
    params: &[crate::ExternalCallableGenericParamInput],
    ty: Option<&crate::ExternalTypeInput>,
) -> TypeId {
    match ty {
        Some(
            ty @ crate::ExternalTypeInput::Enum {
                path: enum_path, ..
            },
        ) => {
            validate_decl_path(ctx, span, "enum", path, enum_path);
            lower_external_type(ctx, span, ty)
        }
        Some(crate::ExternalTypeInput::Alias {
            path: alias_path,
            target,
        }) => {
            validate_decl_path(ctx, span, "alias", path, alias_path);
            if let Some(ty) = ctx.external_type_paths.get(path).copied() {
                return ty;
            }
            let ty = lower_external_type(ctx, span, target);
            ctx.external_type_paths.insert(path.to_vec(), ty);
            ty
        }
        Some(crate::ExternalTypeInput::Nominal {
            path: nominal_path,
            representation,
        }) => {
            validate_decl_path(ctx, span, "nominal type", path, nominal_path);
            let representation = representation
                .as_ref()
                .map(|representation| lower_external_type(ctx, span, representation));
            let ty = complete_external_nominal(ctx, path, params, representation);
            ctx.external_type_paths.insert(path.to_vec(), ty);
            ty
        }
        Some(representation) => {
            let representation = lower_external_type(ctx, span, representation);
            let ty = complete_external_nominal(ctx, path, params, Some(representation));
            ctx.external_type_paths.insert(path.to_vec(), ty);
            ty
        }
        None => {
            let ty = ctx.interner.intern(Type::Nominal(NominalTypeRef {
                read_only_erased_view: false,
                name: path.join("."),
                params: params.iter().map(|param| param.name.clone()).collect(),
                representation: None,
            }));
            ctx.external_type_paths.insert(path.to_vec(), ty);
            ty
        }
    }
}

fn complete_external_nominal(
    ctx: &mut TypePipelineContext<'_>,
    path: &[String],
    params: &[crate::ExternalCallableGenericParamInput],
    representation: Option<TypeId>,
) -> TypeId {
    if let Some(id) = ctx.external_type_paths.get(path).copied() {
        ctx.interner.define_nominal(id, representation);
        id
    } else {
        ctx.interner.intern(Type::Nominal(NominalTypeRef {
            read_only_erased_view: false,
            name: path.join("."),
            params: params.iter().map(|param| param.name.clone()).collect(),
            representation,
        }))
    }
}

pub fn lower_external_effect_row(
    ctx: &mut TypePipelineContext<'_>,
    span: Span,
    row: &crate::ExternalEffectRowInput,
) -> EffectRowRef {
    EffectRowRef {
        effects: row
            .effects
            .iter()
            .map(|effect| EffectRef {
                name: effect.path.join("."),
                args: effect
                    .args
                    .iter()
                    .map(|arg| lower_external_effect_arg(ctx, span, arg))
                    .collect(),
            })
            .collect(),
        tail: row.tail.clone(),
    }
}

fn lower_external_effect_arg(
    ctx: &mut TypePipelineContext<'_>,
    span: Span,
    arg: &crate::ExternalEffectArgInput,
) -> EffectArgRef {
    match arg {
        crate::ExternalEffectArgInput::Type(ty) => {
            EffectArgRef::Type(lower_external_type(ctx, span, ty))
        }
        crate::ExternalEffectArgInput::Path(path) => EffectArgRef::Path(path.clone()),
        crate::ExternalEffectArgInput::String(value) => EffectArgRef::String(value.clone()),
        crate::ExternalEffectArgInput::Int(value) => EffectArgRef::Int(value.clone()),
        crate::ExternalEffectArgInput::Wildcard => EffectArgRef::Wildcard,
    }
}

#[derive(Clone, Debug)]
pub struct ExternalMetadataLoweringError {
    pub message: String,
}

pub use checked_output::CheckedExternalTypeResolver;
mod checked_output;

pub fn lower_external_effect_row_from_output(
    types: &TypeOutput,
    row: &crate::ExternalEffectRowInput,
) -> Result<EffectRowRef, ExternalMetadataLoweringError> {
    CheckedExternalTypeResolver::new(types).effect_row(row)
}

pub fn lower_external_effect_arg_from_output(
    types: &TypeOutput,
    arg: &crate::ExternalEffectArgInput,
) -> Result<EffectArgRef, ExternalMetadataLoweringError> {
    CheckedExternalTypeResolver::new(types).effect_arg(arg)
}

pub fn external_type_id_from_output(
    types: &TypeOutput,
    ty: &crate::ExternalTypeInput,
) -> Result<TypeId, ExternalMetadataLoweringError> {
    CheckedExternalTypeResolver::new(types).type_id(ty)
}

#[cfg(test)]
fn external_type_structural_from_output(
    types: &TypeOutput,
    ty: &crate::ExternalTypeInput,
) -> Result<Type, ExternalMetadataLoweringError> {
    CheckedExternalTypeResolver::new(types).type_structure(ty)
}

fn find_type(store: &TypeStore, needle: &Type) -> Result<TypeId, ExternalMetadataLoweringError> {
    store
        .iter()
        .find_map(|(id, ty)| (ty == needle).then_some(id))
        .ok_or_else(|| ExternalMetadataLoweringError {
            message:
                "external effect metadata references a type that is missing from checked type facts"
                    .to_owned(),
        })
}

fn external_type_by_path(store: &TypeStore, path: &[String]) -> Option<TypeId> {
    let name = path.join(".");
    store.iter().find_map(|(id, ty)| match ty {
        Type::Nominal(nominal) if nominal.name == name => Some(id),
        Type::Enum(declaration) if declaration.name == name => Some(id),
        Type::Named(named) if named.name == name => Some(id),
        _ => None,
    })
}

pub fn is_valid_external_primitive(name: &str) -> bool {
    primitive_from_external_name(name).is_some()
}

pub(crate) fn primitive_from_external_name(name: &str) -> Option<PrimitiveType> {
    Some(match name {
        "bool" => PrimitiveType::Bool,
        "i8" => PrimitiveType::I8,
        "i16" => PrimitiveType::I16,
        "i32" => PrimitiveType::I32,
        "i64" => PrimitiveType::I64,
        "i128" => PrimitiveType::I128,
        "isize" => PrimitiveType::ISize,
        "u8" => PrimitiveType::U8,
        "u16" => PrimitiveType::U16,
        "u32" => PrimitiveType::U32,
        "u64" => PrimitiveType::U64,
        "u128" => PrimitiveType::U128,
        "usize" => PrimitiveType::USize,
        "f32" => PrimitiveType::F32,
        "f64" => PrimitiveType::F64,
        "char" => PrimitiveType::Char,
        "string" => PrimitiveType::String,
        "bytes" => PrimitiveType::Bytes,
        "unit" => PrimitiveType::Unit,
        "never" => PrimitiveType::Never,
        _ => return None,
    })
}

pub(crate) fn external_trust_wrapper(wrapper: &str) -> Option<TrustWrapper> {
    Some(match wrapper {
        "Trusted" | "trusted" => TrustWrapper::Trusted,
        "Secret" | "secret" => TrustWrapper::Secret,
        "Public" | "public" => TrustWrapper::Public,
        "Sanitized" | "sanitized" => TrustWrapper::Sanitized,
        "Untrusted" | "untrusted" => TrustWrapper::Untrusted,
        _ => return None,
    })
}

fn primitive_from_name(ctx: &mut TypePipelineContext<'_>, span: Span, name: &str) -> TypeId {
    let primitive = match name {
        "bool" => PrimitiveType::Bool,
        "i8" => PrimitiveType::I8,
        "i16" => PrimitiveType::I16,
        "i32" => PrimitiveType::I32,
        "i64" => PrimitiveType::I64,
        "i128" => PrimitiveType::I128,
        "isize" => PrimitiveType::ISize,
        "u8" => PrimitiveType::U8,
        "u16" => PrimitiveType::U16,
        "u32" => PrimitiveType::U32,
        "u64" => PrimitiveType::U64,
        "u128" => PrimitiveType::U128,
        "usize" => PrimitiveType::USize,
        "f32" => PrimitiveType::F32,
        "f64" => PrimitiveType::F64,
        "char" => PrimitiveType::Char,
        "string" => PrimitiveType::String,
        "bytes" => PrimitiveType::Bytes,
        "unit" => PrimitiveType::Unit,
        "never" => PrimitiveType::Never,
        _ => {
            invalid_external_metadata(
                ctx,
                span,
                format!("invalid external package metadata: unknown primitive type `{name}`"),
            );
            return ctx.interner.primitive(PrimitiveType::Never);
        }
    };
    ctx.interner.primitive(primitive)
}

fn validate_decl_path(
    ctx: &mut TypePipelineContext<'_>,
    span: Span,
    kind: &str,
    expected: &[String],
    actual: &[String],
) {
    if expected == actual {
        return;
    }
    invalid_external_metadata(
        ctx,
        span,
        format!(
            "invalid external package metadata: {kind} declaration path `{}` does not match exported path `{}`",
            actual.join("."),
            expected.join(".")
        ),
    );
}

fn invalid_external_metadata(ctx: &mut TypePipelineContext<'_>, span: Span, message: String) {
    ctx.diagnostics.push(Diagnostic::type_check(
        TypeDiagnosticCode::IncompleteTypeFacts,
        span,
        message,
    ));
}

#[cfg(test)]
mod tests {
    use super::external_type_structural_from_output;

    #[test]
    fn unknown_external_trust_wrapper_rejects_metadata() {
        let types = crate::TypeOutput::default();
        let input = crate::ExternalTypeInput::Trust {
            wrapper: "MaybeTrusted".to_owned(),
            inner: Box::new(crate::ExternalTypeInput::Primitive("string".to_owned())),
        };

        let error = external_type_structural_from_output(&types, &input)
            .expect_err("unknown trust metadata must fail closed");
        assert!(
            error
                .message
                .contains("unknown trust wrapper `MaybeTrusted`")
        );
    }
}
