use super::{
    ExternalMetadataLoweringError, external_trust_wrapper, external_type_by_path, find_type,
    primitive_from_external_name,
};
use crate::{
    EffectArgRef, EffectRef, EffectRowRef, FieldType, FlowType, HandlerProducedEffects,
    HandlerType, NamedTypeRef, RecordType, ResourceHandleType, Type, TypeConstructorId, TypeId,
    TypeOutput,
};
use std::collections::HashMap;

pub struct CheckedExternalTypeResolver<'a> {
    types: &'a TypeOutput,
    parameters: Option<HashMap<String, TypeId>>,
}

impl<'a> CheckedExternalTypeResolver<'a> {
    pub fn new(types: &'a TypeOutput) -> Self {
        Self {
            types,
            parameters: None,
        }
    }

    pub fn for_callable(
        types: &'a TypeOutput,
        parameters: &[crate::CallableGenericParam],
    ) -> Result<Self, ExternalMetadataLoweringError> {
        let mut bindings = HashMap::new();
        for param in parameters
            .iter()
            .filter(|param| param.kind == crate::CallableGenericParamKind::Type)
        {
            if types.store.get(param.subject).is_none()
                || bindings.insert(param.name.clone(), param.subject).is_some()
            {
                return Err(ExternalMetadataLoweringError { message: "callable generic scope has missing or duplicate checked type parameter identities".into() });
            }
        }
        Ok(Self {
            types,
            parameters: Some(bindings),
        })
    }

    pub fn effect_row(
        &self,
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
                            .map(|arg| self.effect_arg(arg))
                            .collect::<Result<Vec<_>, _>>()?,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
            tail: row.tail.clone(),
        })
    }

    pub fn effect_arg(
        &self,
        arg: &crate::ExternalEffectArgInput,
    ) -> Result<EffectArgRef, ExternalMetadataLoweringError> {
        match arg {
            crate::ExternalEffectArgInput::Type(ty) => Ok(EffectArgRef::Type(self.type_id(ty)?)),
            crate::ExternalEffectArgInput::Path(path) => Ok(EffectArgRef::Path(path.clone())),
            crate::ExternalEffectArgInput::String(value) => Ok(EffectArgRef::String(value.clone())),
            crate::ExternalEffectArgInput::Int(value) => Ok(EffectArgRef::Int(value.clone())),
            crate::ExternalEffectArgInput::Wildcard => Ok(EffectArgRef::Wildcard),
        }
    }

    pub fn type_id(
        &self,
        ty: &crate::ExternalTypeInput,
    ) -> Result<TypeId, ExternalMetadataLoweringError> {
        match ty {
        crate::ExternalTypeInput::Var(name) if self.parameters.is_some() => {
            self.parameters.as_ref().and_then(|params| params.get(name)).copied()
                .ok_or_else(|| ExternalMetadataLoweringError {
                    message: format!("external type parameter `{name}` is not declared in the checked callable scope"),
                })
        }
        crate::ExternalTypeInput::Alias { target, .. } => {
            self.type_id(target)
        }
        crate::ExternalTypeInput::Named(path)
        | crate::ExternalTypeInput::Nominal { path, .. }
        | crate::ExternalTypeInput::Enum { path, .. }
        | crate::ExternalTypeInput::Applied { path, .. } => {
            if let Some((parameters, target)) = self.types.facts.external_type_aliases.get(path) {
                let args = match ty {
                    crate::ExternalTypeInput::Applied { args, .. } => args.as_slice(),
                    crate::ExternalTypeInput::Named(_) => &[],
                    _ => return Err(ExternalMetadataLoweringError { message: "external nominal or enum type conflicts with a checked alias".into() }),
                };
                if args.len() != parameters.len() {
                    return Err(ExternalMetadataLoweringError { message: format!("external alias `{}` expects {} arguments, got {}", path.join("."), parameters.len(), args.len()) });
                }
                let bindings = parameters.iter().cloned().zip(args.iter().map(|arg| self.type_id(arg)).collect::<Result<Vec<_>, _>>()?).collect();
                return crate::substitute_named_params_in_store(&self.types.store, *target, &bindings).map_err(|error| ExternalMetadataLoweringError { message: format!("external alias `{}` has no checked instantiation: {error}", path.join(".")) });
            }
            let Some(id) = external_type_by_path(&self.types.store, path) else {
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
                    .map(|arg| self.type_id(arg))
                    .collect::<Result<Vec<_>, _>>()?;
                return find_type(
                    &self.types.store,
                    &Type::Applied {
                        constructor: TypeConstructorId(id.0),
                        args,
                    },
                );
            }
            Ok(id)
        }
        _ => {
            let structural = self.type_structure(ty)?;
            find_type(&self.types.store, &structural)
        }
    }
    }

    pub(super) fn type_structure(
        &self,
        ty: &crate::ExternalTypeInput,
    ) -> Result<Type, ExternalMetadataLoweringError> {
        match ty {
            crate::ExternalTypeInput::Primitive(name) => Ok(Type::Primitive(
                primitive_from_external_name(name).ok_or_else(|| {
                    ExternalMetadataLoweringError {
                        message: format!(
                            "external effect metadata references unknown primitive type `{name}`"
                        ),
                    }
                })?,
            )),
            crate::ExternalTypeInput::Var(name) => Ok(Type::Named(NamedTypeRef {
                source_parameter: None,
                name: name.clone(),
            })),
            crate::ExternalTypeInput::Alias { target, .. } => self.type_structure(target),
            crate::ExternalTypeInput::Named(_)
            | crate::ExternalTypeInput::Nominal { .. }
            | crate::ExternalTypeInput::Enum { .. }
            | crate::ExternalTypeInput::Applied { .. } => {
                let id = self.type_id(ty)?;
                self.types
                    .store
                    .get(id)
                    .cloned()
                    .ok_or_else(|| ExternalMetadataLoweringError {
                        message: "external effect metadata resolved to missing checked type id"
                            .to_owned(),
                    })
            }
            crate::ExternalTypeInput::Array(inner) => Ok(Type::Array(self.type_id(inner)?)),
            crate::ExternalTypeInput::List(inner) => Ok(Type::List(self.type_id(inner)?)),
            crate::ExternalTypeInput::Map { key, value } => Ok(Type::Map {
                key: self.type_id(key)?,
                value: self.type_id(value)?,
            }),
            crate::ExternalTypeInput::Set(inner) => Ok(Type::Set(self.type_id(inner)?)),
            crate::ExternalTypeInput::Range(inner) => Ok(Type::Range {
                index: self.type_id(inner)?,
            }),
            crate::ExternalTypeInput::Slice(inner) => Ok(Type::Slice(self.type_id(inner)?)),
            crate::ExternalTypeInput::Option(inner) => Ok(Type::Option(self.type_id(inner)?)),
            crate::ExternalTypeInput::Result { ok, err } => Ok(Type::Result {
                ok: self.type_id(ok)?,
                err: self.type_id(err)?,
            }),
            crate::ExternalTypeInput::Record { fields } => Ok(Type::Record(RecordType {
                fields: fields
                    .iter()
                    .map(|field| {
                        Ok(FieldType {
                            name: field.name.clone(),
                            ty: self.type_id(&field.ty)?,
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            })),
            crate::ExternalTypeInput::Tuple(elements) => Ok(Type::Tuple(
                elements
                    .iter()
                    .map(|element| self.type_id(element))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            crate::ExternalTypeInput::Function {
                input,
                output,
                effects,
            } => Ok(Type::Function(FlowType {
                input: input
                    .iter()
                    .map(|input| self.type_id(input))
                    .collect::<Result<Vec<_>, _>>()?,
                output: self.type_id(output)?,
                effects: effects
                    .as_ref()
                    .map(|row| self.effect_row(row))
                    .transpose()?,
            })),
            crate::ExternalTypeInput::Handler {
                handled,
                produced,
                result,
            } => Ok(Type::Handler(HandlerType {
                handled: self.effect_row(handled)?,
                produced: produced
                    .as_ref()
                    .map(|row| self.effect_row(row))
                    .transpose()?
                    .map(HandlerProducedEffects::Explicit)
                    .unwrap_or(HandlerProducedEffects::Infer),
                result: result
                    .as_ref()
                    .map(|result| self.type_id(result))
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
                inner: self.type_id(inner)?,
            }),
            crate::ExternalTypeInput::Prompt => Ok(Type::Prompt),
            crate::ExternalTypeInput::PromptPart => Ok(Type::PromptPart),
            crate::ExternalTypeInput::Message(inner) => Ok(Type::Message(self.type_id(inner)?)),
            crate::ExternalTypeInput::MemorySelection(inner) => {
                Ok(Type::MemorySelection(self.type_id(inner)?))
            }
            crate::ExternalTypeInput::Store { key, value } => Ok(Type::Store {
                key: self.type_id(key)?,
                value: self.type_id(value)?,
            }),
            crate::ExternalTypeInput::MemoryRegion(inner) => {
                Ok(Type::MemoryRegion(self.type_id(inner)?))
            }
            crate::ExternalTypeInput::ResourceHandle { name, args } => {
                Ok(Type::ResourceHandle(ResourceHandleType::Other {
                    name: name.clone(),
                    args: args
                        .iter()
                        .map(|arg| self.type_id(arg))
                        .collect::<Result<Vec<_>, _>>()?,
                }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CallableGenericParam, CallableGenericParamKind, ExternalTypeInput, TypeInterner};

    #[test]
    fn checked_effect_selector_instantiates_external_alias_in_callable_scope() {
        let mut interner = TypeInterner::new();
        let alias_param = interner.intern(Type::Named(NamedTypeRef {
            name: "A".into(),
            source_parameter: None,
        }));
        let caller_param = interner.intern(Type::Named(NamedTypeRef {
            name: "T".into(),
            source_parameter: Some(etas_hir::SymbolId(10)),
        }));
        let target = interner.intern(Type::Array(alias_param));
        let expected = interner.intern(Type::Array(caller_param));
        let path = vec!["library".into(), "ArrayAlias".into()];
        let mut facts = crate::TypeFacts::default();
        facts
            .external_type_aliases
            .insert(path.clone(), (vec!["A".into()], target));
        let types = TypeOutput {
            facts: std::sync::Arc::new(facts),
            store: std::sync::Arc::new(interner.into_store()),
            ..Default::default()
        };
        let parameter = CallableGenericParam {
            name: "T".into(),
            subject: caller_param,
            kind: CallableGenericParamKind::Type,
            bounds: vec![],
        };
        let resolver = CheckedExternalTypeResolver::for_callable(&types, &[parameter]).unwrap();
        let arg = crate::ExternalEffectArgInput::Type(ExternalTypeInput::Applied {
            path: path.clone(),
            args: vec![ExternalTypeInput::Var("T".into())],
        });
        assert_eq!(
            resolver.effect_arg(&arg).unwrap(),
            EffectArgRef::Type(expected)
        );
        let wrong = ExternalTypeInput::Applied { path, args: vec![] };
        assert!(resolver.type_id(&wrong).is_err());
    }

    #[test]
    fn checked_callable_scope_distinguishes_same_spelled_binders_and_rejects_undeclared_variables()
    {
        let mut interner = TypeInterner::new();
        let first = interner.intern(Type::Named(NamedTypeRef {
            name: "T".into(),
            source_parameter: Some(etas_hir::SymbolId(10)),
        }));
        let second = interner.intern(Type::Named(NamedTypeRef {
            name: "T".into(),
            source_parameter: Some(etas_hir::SymbolId(11)),
        }));
        interner.intern(Type::Named(NamedTypeRef {
            name: "T".into(),
            source_parameter: None,
        }));
        interner.intern(Type::Named(NamedTypeRef {
            name: "U".into(),
            source_parameter: None,
        }));
        let arrays = [
            interner.intern(Type::Array(first)),
            interner.intern(Type::Array(second)),
        ];
        let types = TypeOutput {
            store: std::sync::Arc::new(interner.into_store()),
            ..Default::default()
        };
        let metadata = ExternalTypeInput::Array(Box::new(ExternalTypeInput::Var("T".into())));
        for (subject, expected) in [(first, arrays[0]), (second, arrays[1])] {
            let param = CallableGenericParam {
                name: "T".into(),
                subject,
                kind: CallableGenericParamKind::Type,
                bounds: vec![],
            };
            let resolver =
                CheckedExternalTypeResolver::for_callable(&types, std::slice::from_ref(&param))
                    .unwrap();
            assert_eq!(resolver.type_id(&metadata).unwrap(), expected);
            assert!(
                resolver
                    .type_id(&ExternalTypeInput::Var("U".into()))
                    .unwrap_err()
                    .message
                    .contains("not declared")
            );
            assert!(
                CheckedExternalTypeResolver::for_callable(&types, &[param.clone(), param]).is_err()
            );
        }
    }
}
