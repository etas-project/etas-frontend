use etas_core::{Diagnostic, SourceId, Span, TextSize, TypeDiagnosticCode};

use crate::{
    EffectActionArgKind, EffectActionSignature, EffectArgRef, EffectRef, EffectRowRef, FieldType,
    FlowType, HandlerProducedEffects, HandlerType, NamedTypeRef, NominalTypeRef, PrimitiveType,
    RecordType, ResourceHandleType, TrustWrapper, Type, TypeConstructorId, TypeId, TypeOutput,
    TypeStore, pipeline::context::TypePipelineContext,
};

pub fn lower_external_action_signature(
    ctx: &mut TypePipelineContext<'_>,
    action: &crate::ExternalActionSignatureInput,
) -> EffectActionSignature {
    validate_external_action_selector_metadata(ctx, action);
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
    EffectActionSignature {
        params: action
            .params
            .iter()
            .map(|ty| lower_external_type(ctx, ty))
            .collect(),
        output: lower_external_type(ctx, &action.output),
        effect_args,
        selector_param_names: action.selector_param_names.clone(),
        selector_defaults: action
            .selector_defaults
            .iter()
            .map(|arg| arg.as_ref().map(|arg| lower_external_effect_arg(ctx, arg)))
            .collect(),
        returns_never: action.returns_never,
    }
}

fn validate_external_action_selector_metadata(
    ctx: &mut TypePipelineContext<'_>,
    action: &crate::ExternalActionSignatureInput,
) {
    if action.selector_param_names.len() != action.effect_args.len() {
        invalid_external_metadata(
            ctx,
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
            format!(
                "invalid external package metadata: action `{}` selector_defaults length {} does not match effect_args length {}",
                action.path.join("."),
                action.selector_defaults.len(),
                action.effect_args.len()
            ),
        );
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
                crate::ExternalEffectArgInput::String(_) | crate::ExternalEffectArgInput::Path(_)
            )
        }
    }
}

pub fn lower_external_type(
    ctx: &mut TypePipelineContext<'_>,
    ty: &crate::ExternalTypeInput,
) -> TypeId {
    match ty {
        crate::ExternalTypeInput::Primitive(name) => primitive_from_name(ctx, name),
        crate::ExternalTypeInput::Var(name) => ctx
            .interner
            .intern(Type::Named(NamedTypeRef { name: name.clone() })),
        crate::ExternalTypeInput::Named(path) => ctx
            .external_type_paths
            .get(path)
            .copied()
            .unwrap_or_else(|| named_external_type(ctx, path)),
        crate::ExternalTypeInput::Applied { path, args } => {
            let constructor = ctx
                .external_type_paths
                .get(path)
                .copied()
                .unwrap_or_else(|| named_external_type(ctx, path));
            let args = args
                .iter()
                .map(|arg| lower_external_type(ctx, arg))
                .collect();
            ctx.interner.intern(Type::Applied {
                constructor: TypeConstructorId(constructor.0),
                args,
            })
        }
        crate::ExternalTypeInput::Alias { target, .. } => lower_external_type(ctx, target),
        crate::ExternalTypeInput::Nominal {
            path,
            representation,
        } => {
            let representation = representation
                .as_ref()
                .map(|representation| lower_external_type(ctx, representation));
            ctx.interner.intern(Type::Nominal(NominalTypeRef {
                name: path.join("."),
                params: Vec::new(),
                representation,
            }))
        }
        crate::ExternalTypeInput::Array(inner) => {
            let inner = lower_external_type(ctx, inner);
            ctx.interner.intern(Type::Array(inner))
        }
        crate::ExternalTypeInput::List(inner) => {
            let inner = lower_external_type(ctx, inner);
            ctx.interner.intern(Type::List(inner))
        }
        crate::ExternalTypeInput::Map { key, value } => {
            let key = lower_external_type(ctx, key);
            let value = lower_external_type(ctx, value);
            ctx.interner.intern(Type::Map { key, value })
        }
        crate::ExternalTypeInput::Set(inner) => {
            let inner = lower_external_type(ctx, inner);
            ctx.interner.intern(Type::Set(inner))
        }
        crate::ExternalTypeInput::Range(inner) => {
            let index = lower_external_type(ctx, inner);
            ctx.interner.intern(Type::Range { index })
        }
        crate::ExternalTypeInput::Slice(inner) => {
            let inner = lower_external_type(ctx, inner);
            ctx.interner.intern(Type::Slice(inner))
        }
        crate::ExternalTypeInput::Option(inner) => {
            let inner = lower_external_type(ctx, inner);
            ctx.interner.intern(Type::Option(inner))
        }
        crate::ExternalTypeInput::Result { ok, err } => {
            let ok = lower_external_type(ctx, ok);
            let err = lower_external_type(ctx, err);
            ctx.interner.intern(Type::Result { ok, err })
        }
        crate::ExternalTypeInput::Record { fields } => {
            let fields = fields
                .iter()
                .map(|field| FieldType {
                    name: field.name.clone(),
                    ty: lower_external_type(ctx, &field.ty),
                })
                .collect();
            ctx.interner.intern(Type::Record(RecordType { fields }))
        }
        crate::ExternalTypeInput::Tuple(elements) => {
            let elements = elements
                .iter()
                .map(|ty| lower_external_type(ctx, ty))
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
                .map(|ty| lower_external_type(ctx, ty))
                .collect();
            let output = lower_external_type(ctx, output);
            let effects = effects
                .as_ref()
                .map(|row| lower_external_effect_row(ctx, row));
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
                .map(|result| lower_external_type(ctx, result));
            let handled = lower_external_effect_row(ctx, handled);
            let produced = produced
                .as_ref()
                .map(|row| lower_external_effect_row(ctx, row))
                .map(HandlerProducedEffects::Explicit)
                .unwrap_or(HandlerProducedEffects::Infer);
            ctx.interner.intern(Type::Handler(HandlerType {
                handled,
                produced,
                result,
            }))
        }
        crate::ExternalTypeInput::Trust { wrapper, inner } => {
            let inner = lower_external_type(ctx, inner);
            let Some(wrapper) = external_trust_wrapper(wrapper) else {
                invalid_external_metadata(
                    ctx,
                    format!("invalid external package metadata: unknown trust wrapper `{wrapper}`"),
                );
                return ctx.interner.primitive(PrimitiveType::Never);
            };
            ctx.interner.intern(Type::Trust { wrapper, inner })
        }
        crate::ExternalTypeInput::Prompt => ctx.interner.intern(Type::Prompt),
        crate::ExternalTypeInput::PromptPart => ctx.interner.intern(Type::PromptPart),
        crate::ExternalTypeInput::Message(inner) => {
            let inner = lower_external_type(ctx, inner);
            ctx.interner.intern(Type::Message(inner))
        }
        crate::ExternalTypeInput::MemorySelection(inner) => {
            let inner = lower_external_type(ctx, inner);
            ctx.interner.intern(Type::MemorySelection(inner))
        }
        crate::ExternalTypeInput::Store { key, value } => {
            let key = lower_external_type(ctx, key);
            let value = lower_external_type(ctx, value);
            ctx.interner.intern(Type::Store { key, value })
        }
        crate::ExternalTypeInput::MemoryRegion(inner) => {
            let inner = lower_external_type(ctx, inner);
            ctx.interner.intern(Type::MemoryRegion(inner))
        }
        crate::ExternalTypeInput::ResourceHandle { name, args } => {
            let args = args.iter().map(|ty| lower_external_type(ctx, ty)).collect();
            ctx.interner
                .intern(Type::ResourceHandle(ResourceHandleType::Other {
                    name: name.clone(),
                    args,
                }))
        }
    }
}

pub fn lower_external_type_declaration(
    ctx: &mut TypePipelineContext<'_>,
    path: &[String],
    ty: Option<&crate::ExternalTypeInput>,
) -> TypeId {
    match ty {
        Some(crate::ExternalTypeInput::Alias {
            path: alias_path,
            target,
        }) => {
            validate_decl_path(ctx, "alias", path, alias_path);
            let ty = lower_external_type(ctx, target);
            ctx.external_type_paths.insert(path.to_vec(), ty);
            ty
        }
        Some(crate::ExternalTypeInput::Nominal {
            path: nominal_path,
            representation,
        }) => {
            validate_decl_path(ctx, "nominal type", path, nominal_path);
            let representation = representation
                .as_ref()
                .map(|representation| lower_external_type(ctx, representation));
            let ty = ctx.interner.intern(Type::Nominal(NominalTypeRef {
                name: path.join("."),
                params: Vec::new(),
                representation,
            }));
            ctx.external_type_paths.insert(path.to_vec(), ty);
            ty
        }
        Some(representation) => {
            let representation = lower_external_type(ctx, representation);
            let ty = ctx.interner.intern(Type::Nominal(NominalTypeRef {
                name: path.join("."),
                params: Vec::new(),
                representation: Some(representation),
            }));
            ctx.external_type_paths.insert(path.to_vec(), ty);
            ty
        }
        None => {
            let ty = ctx.interner.intern(Type::Nominal(NominalTypeRef {
                name: path.join("."),
                params: Vec::new(),
                representation: None,
            }));
            ctx.external_type_paths.insert(path.to_vec(), ty);
            ty
        }
    }
}

pub fn lower_external_effect_row(
    ctx: &mut TypePipelineContext<'_>,
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
                    .map(|arg| lower_external_effect_arg(ctx, arg))
                    .collect(),
            })
            .collect(),
        tail: None,
    }
}

fn lower_external_effect_arg(
    ctx: &mut TypePipelineContext<'_>,
    arg: &crate::ExternalEffectArgInput,
) -> EffectArgRef {
    match arg {
        crate::ExternalEffectArgInput::Type(ty) => EffectArgRef::Type(lower_external_type(ctx, ty)),
        crate::ExternalEffectArgInput::Path(path) => EffectArgRef::Path(path.clone()),
        crate::ExternalEffectArgInput::String(value) => EffectArgRef::String(value.clone()),
        crate::ExternalEffectArgInput::Wildcard => EffectArgRef::Wildcard,
    }
}

#[derive(Clone, Debug)]
pub struct ExternalMetadataLoweringError {
    pub message: String,
}

pub fn lower_external_effect_row_from_output(
    types: &TypeOutput,
    row: &crate::ExternalEffectRowInput,
) -> Result<EffectRowRef, ExternalMetadataLoweringError> {
    Ok(EffectRowRef {
        effects: row
            .effects
            .iter()
            .map(|effect| {
                Ok(EffectRef {
                    name: effect.path.join("."),
                    args: effect
                        .args
                        .iter()
                        .map(|arg| lower_external_effect_arg_from_output(types, arg))
                        .collect::<Result<Vec<_>, _>>()?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        tail: None,
    })
}

pub fn lower_external_effect_arg_from_output(
    types: &TypeOutput,
    arg: &crate::ExternalEffectArgInput,
) -> Result<EffectArgRef, ExternalMetadataLoweringError> {
    match arg {
        crate::ExternalEffectArgInput::Type(ty) => {
            Ok(EffectArgRef::Type(external_type_id_from_output(types, ty)?))
        }
        crate::ExternalEffectArgInput::Path(path) => Ok(EffectArgRef::Path(path.clone())),
        crate::ExternalEffectArgInput::String(value) => Ok(EffectArgRef::String(value.clone())),
        crate::ExternalEffectArgInput::Wildcard => Ok(EffectArgRef::Wildcard),
    }
}

pub fn external_type_id_from_output(
    types: &TypeOutput,
    ty: &crate::ExternalTypeInput,
) -> Result<TypeId, ExternalMetadataLoweringError> {
    match ty {
        crate::ExternalTypeInput::Alias { target, .. } => {
            external_type_id_from_output(types, target)
        }
        crate::ExternalTypeInput::Named(path)
        | crate::ExternalTypeInput::Nominal { path, .. }
        | crate::ExternalTypeInput::Applied { path, .. } => {
            let Some(id) = external_type_by_path(&types.store, path) else {
                return Err(ExternalMetadataLoweringError {
                    message: format!(
                        "external effect metadata references missing checked type `{}`",
                        path.join(".")
                    ),
                });
            };
            if let crate::ExternalTypeInput::Applied { args, .. } = ty {
                let args = args
                    .iter()
                    .map(|arg| external_type_id_from_output(types, arg))
                    .collect::<Result<Vec<_>, _>>()?;
                return find_type(
                    &types.store,
                    &Type::Applied {
                        constructor: TypeConstructorId(id.0),
                        args,
                    },
                );
            }
            Ok(id)
        }
        _ => {
            let structural = external_type_structural_from_output(types, ty)?;
            find_type(&types.store, &structural)
        }
    }
}

fn external_type_structural_from_output(
    types: &TypeOutput,
    ty: &crate::ExternalTypeInput,
) -> Result<Type, ExternalMetadataLoweringError> {
    match ty {
        crate::ExternalTypeInput::Primitive(name) => Ok(Type::Primitive(
            primitive_from_external_name(name).ok_or_else(|| ExternalMetadataLoweringError {
                message: format!(
                    "external effect metadata references unknown primitive type `{name}`"
                ),
            })?,
        )),
        crate::ExternalTypeInput::Var(name) => Ok(Type::Named(NamedTypeRef { name: name.clone() })),
        crate::ExternalTypeInput::Alias { target, .. } => {
            external_type_structural_from_output(types, target)
        }
        crate::ExternalTypeInput::Named(_)
        | crate::ExternalTypeInput::Nominal { .. }
        | crate::ExternalTypeInput::Applied { .. } => {
            let id = external_type_id_from_output(types, ty)?;
            types
                .store
                .get(id)
                .cloned()
                .ok_or_else(|| ExternalMetadataLoweringError {
                    message: "external effect metadata resolved to missing checked type id"
                        .to_owned(),
                })
        }
        crate::ExternalTypeInput::Array(inner) => {
            Ok(Type::Array(external_type_id_from_output(types, inner)?))
        }
        crate::ExternalTypeInput::List(inner) => {
            Ok(Type::List(external_type_id_from_output(types, inner)?))
        }
        crate::ExternalTypeInput::Map { key, value } => Ok(Type::Map {
            key: external_type_id_from_output(types, key)?,
            value: external_type_id_from_output(types, value)?,
        }),
        crate::ExternalTypeInput::Set(inner) => {
            Ok(Type::Set(external_type_id_from_output(types, inner)?))
        }
        crate::ExternalTypeInput::Range(inner) => Ok(Type::Range {
            index: external_type_id_from_output(types, inner)?,
        }),
        crate::ExternalTypeInput::Slice(inner) => {
            Ok(Type::Slice(external_type_id_from_output(types, inner)?))
        }
        crate::ExternalTypeInput::Option(inner) => {
            Ok(Type::Option(external_type_id_from_output(types, inner)?))
        }
        crate::ExternalTypeInput::Result { ok, err } => Ok(Type::Result {
            ok: external_type_id_from_output(types, ok)?,
            err: external_type_id_from_output(types, err)?,
        }),
        crate::ExternalTypeInput::Record { fields } => Ok(Type::Record(RecordType {
            fields: fields
                .iter()
                .map(|field| {
                    Ok(FieldType {
                        name: field.name.clone(),
                        ty: external_type_id_from_output(types, &field.ty)?,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
        })),
        crate::ExternalTypeInput::Tuple(elements) => Ok(Type::Tuple(
            elements
                .iter()
                .map(|element| external_type_id_from_output(types, element))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        crate::ExternalTypeInput::Function {
            input,
            output,
            effects,
        } => Ok(Type::Function(FlowType {
            input: input
                .iter()
                .map(|input| external_type_id_from_output(types, input))
                .collect::<Result<Vec<_>, _>>()?,
            output: external_type_id_from_output(types, output)?,
            effects: effects
                .as_ref()
                .map(|row| lower_external_effect_row_from_output(types, row))
                .transpose()?,
        })),
        crate::ExternalTypeInput::Handler {
            handled,
            produced,
            result,
        } => Ok(Type::Handler(HandlerType {
            handled: lower_external_effect_row_from_output(types, handled)?,
            produced: produced
                .as_ref()
                .map(|row| lower_external_effect_row_from_output(types, row))
                .transpose()?
                .map(HandlerProducedEffects::Explicit)
                .unwrap_or(HandlerProducedEffects::Infer),
            result: result
                .as_ref()
                .map(|result| external_type_id_from_output(types, result))
                .transpose()?,
        })),
        crate::ExternalTypeInput::Trust { wrapper, inner } => Ok(Type::Trust {
            wrapper: external_trust_wrapper(wrapper).ok_or_else(|| {
                ExternalMetadataLoweringError {
                    message: format!(
                        "external metadata contains unknown trust wrapper `{wrapper}`"
                    ),
                }
            })?,
            inner: external_type_id_from_output(types, inner)?,
        }),
        crate::ExternalTypeInput::Prompt => Ok(Type::Prompt),
        crate::ExternalTypeInput::PromptPart => Ok(Type::PromptPart),
        crate::ExternalTypeInput::Message(inner) => {
            Ok(Type::Message(external_type_id_from_output(types, inner)?))
        }
        crate::ExternalTypeInput::MemorySelection(inner) => Ok(Type::MemorySelection(
            external_type_id_from_output(types, inner)?,
        )),
        crate::ExternalTypeInput::Store { key, value } => Ok(Type::Store {
            key: external_type_id_from_output(types, key)?,
            value: external_type_id_from_output(types, value)?,
        }),
        crate::ExternalTypeInput::MemoryRegion(inner) => Ok(Type::MemoryRegion(
            external_type_id_from_output(types, inner)?,
        )),
        crate::ExternalTypeInput::ResourceHandle { name, args } => {
            Ok(Type::ResourceHandle(ResourceHandleType::Other {
                name: name.clone(),
                args: args
                    .iter()
                    .map(|arg| external_type_id_from_output(types, arg))
                    .collect::<Result<Vec<_>, _>>()?,
            }))
        }
    }
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
        Type::Named(named) if named.name == name => Some(id),
        _ => None,
    })
}

fn primitive_from_external_name(name: &str) -> Option<PrimitiveType> {
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

fn external_trust_wrapper(wrapper: &str) -> Option<TrustWrapper> {
    Some(match wrapper {
        "Trusted" | "trusted" => TrustWrapper::Trusted,
        "Secret" | "secret" => TrustWrapper::Secret,
        "Public" | "public" => TrustWrapper::Public,
        "Sanitized" | "sanitized" => TrustWrapper::Sanitized,
        "Untrusted" | "untrusted" => TrustWrapper::Untrusted,
        _ => return None,
    })
}

fn primitive_from_name(ctx: &mut TypePipelineContext<'_>, name: &str) -> TypeId {
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
                format!("invalid external package metadata: unknown primitive type `{name}`"),
            );
            return ctx.interner.primitive(PrimitiveType::Never);
        }
    };
    ctx.interner.primitive(primitive)
}

fn validate_decl_path(
    ctx: &mut TypePipelineContext<'_>,
    kind: &str,
    expected: &[String],
    actual: &[String],
) {
    if expected == actual {
        return;
    }
    invalid_external_metadata(
        ctx,
        format!(
            "invalid external package metadata: {kind} declaration path `{}` does not match exported path `{}`",
            actual.join("."),
            expected.join(".")
        ),
    );
}

fn invalid_external_metadata(ctx: &mut TypePipelineContext<'_>, message: String) {
    ctx.diagnostics.push(Diagnostic::type_check(
        TypeDiagnosticCode::IncompleteTypeFacts,
        Span::empty(SourceId(0), TextSize::ZERO),
        message,
    ));
}

fn named_external_type(ctx: &mut TypePipelineContext<'_>, path: &[String]) -> TypeId {
    ctx.interner.intern(Type::Named(NamedTypeRef {
        name: path.join("."),
    }))
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
