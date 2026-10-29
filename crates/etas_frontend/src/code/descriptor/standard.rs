use etas_code::{DefinitionKey, descriptor::*};
use etas_std::{StdDecl, StdTrustWrapper, StdType, TypeDeclKind};

use super::{DefinitionDescriptorProjector, ProjectionError};
use crate::code::identity::TypeDeclaration;

impl DefinitionDescriptorProjector<'_> {
    pub(super) fn standard_declaration(
        &self,
        name: &str,
        module: &[String],
    ) -> Result<TypeDeclaration, ProjectionError> {
        let path = name.split('.').collect::<Vec<_>>();
        let symbol = if path.len() > 1 {
            self.checked.std_registry.lookup_qualified(&path)
        } else {
            let mut scoped = module.to_vec();
            scoped.push(name.into());
            self.checked
                .std_registry
                .lookup_qualified(&scoped)
                .or_else(|| {
                    self.checked
                        .std_registry
                        .lookup_prelude_in_namespace(name, etas_std::StdNamespace::Type)
                })
        }
        .ok_or_else(|| ProjectionError::MissingIdentity(name.into()))?;
        self.identities
            .type_names
            .get(&symbol.qualified_path.join("."))
            .cloned()
            .ok_or_else(|| ProjectionError::MissingIdentity(name.into()))
    }

    pub(super) fn project_standard(
        &mut self,
        source: StdType,
        scope: usize,
        module: &[String],
    ) -> Result<TypeInfo, ProjectionError> {
        let (constructor, sources) = match source {
            StdType::Primitive(primitive) => {
                return Ok(TypeInfo::Primitive(primitive_info(primitive)));
            }
            StdType::Var(name) => {
                let binder = self.scopes[scope]
                    .binders
                    .get(&name)
                    .copied()
                    .ok_or(ProjectionError::UnresolvedBinder(name))?;
                if self.binders[binder.0 as usize].kind != GenericParameterKind::Type {
                    return Err(ProjectionError::Invalid(
                        "standard value type refers to an effect-row binder".into(),
                    ));
                }
                return Ok(TypeInfo::Parameter(binder));
            }
            StdType::Record(fields) => {
                let mut projected = Vec::with_capacity(fields.len());
                for field in fields {
                    self.text(&field.name)?;
                    projected.push(FieldInfo {
                        name: field.name,
                        ty: self.standard_ref(field.ty, scope, module)?,
                    });
                }
                return Ok(TypeInfo::Record(projected));
            }
            StdType::Tuple(elements) => {
                return Ok(TypeInfo::Tuple(
                    elements
                        .into_iter()
                        .map(|ty| self.standard_ref(ty, scope, module))
                        .collect::<Result<_, _>>()?,
                ));
            }
            StdType::Function {
                input,
                output,
                effects,
            } => {
                let parameters = input
                    .into_iter()
                    .map(|ty| {
                        Ok(ParameterInfo {
                            name: None,
                            ty: self.standard_ref(ty, scope, module)?,
                        })
                    })
                    .collect::<Result<_, ProjectionError>>()?;
                let output = self.standard_ref(*output, scope, module)?;
                let mut projected = Vec::with_capacity(effects.len());
                for effect in effects {
                    let target = self
                        .identities
                        .effect_target(&self.checked, &effect.path.join("."))?;
                    let mut arguments = Vec::with_capacity(effect.args.len());
                    for arg in effect.args {
                        self.edge()?;
                        arguments.push(match arg {
                            etas_std::StdStaticArg::Type(ty) => {
                                StaticEffectArgInfo::Type(self.standard_ref(ty, scope, module)?)
                            }
                            etas_std::StdStaticArg::Path(path) => {
                                match self.identities.callable_path(&path)? {
                                    Some(id) => StaticEffectArgInfo::Callable(id.clone()),
                                    None => StaticEffectArgInfo::Resource(
                                        self.identities
                                            .path(&path)
                                            .ok_or_else(|| {
                                                ProjectionError::MissingIdentity(path.join("."))
                                            })?
                                            .clone(),
                                    ),
                                }
                            }
                            etas_std::StdStaticArg::String(text) => {
                                self.text(&text)?;
                                StaticEffectArgInfo::String(text)
                            }
                            etas_std::StdStaticArg::Int(text) => {
                                self.text(&text)?;
                                StaticEffectArgInfo::Integer(text)
                            }
                            etas_std::StdStaticArg::Wildcard => StaticEffectArgInfo::Wildcard,
                        });
                    }
                    projected.push(EffectInfo { target, arguments });
                }
                self.edge()?;
                self.node()?;
                let effects = EffectRowInfoRef(self.rows.len() as u32);
                self.rows.push(Some(EffectRowInfo {
                    effects: projected,
                    tail: None,
                }));
                return Ok(TypeInfo::Function(SignatureInfo {
                    generics: Vec::new(),
                    parameters,
                    output,
                    declared_output: None,
                    effects: Some(effects),
                }));
            }
            StdType::NamedApplied { name, args } => {
                let constructor = self.standard_ref(StdType::Named(name), scope, module)?;
                let arguments = args
                    .into_iter()
                    .map(|ty| self.standard_ref(ty, scope, module))
                    .collect::<Result<_, _>>()?;
                return Ok(TypeInfo::Applied {
                    constructor,
                    arguments,
                });
            }
            StdType::Named(_) => {
                return Err(ProjectionError::Invalid(
                    "named std type was not resolved before projection".into(),
                ));
            }
            StdType::Support(_) => {
                return Err(ProjectionError::Invalid(
                    "std support spec cannot be reflected as a value type".into(),
                ));
            }
            StdType::Array(ty) => (BuiltinTypeInfo::Array, vec![*ty]),
            StdType::List(ty) => (BuiltinTypeInfo::List, vec![*ty]),
            StdType::Map { key, value } => (BuiltinTypeInfo::Map, vec![*key, *value]),
            StdType::Set(ty) => (BuiltinTypeInfo::Set, vec![*ty]),
            StdType::Range(ty) => (BuiltinTypeInfo::Range, vec![*ty]),
            StdType::Slice(ty) => (BuiltinTypeInfo::Slice, vec![*ty]),
            StdType::Option(ty) => (BuiltinTypeInfo::Option, vec![*ty]),
            StdType::Result { ok, err } => (BuiltinTypeInfo::Result, vec![*ok, *err]),
            StdType::Schema(ty) => (BuiltinTypeInfo::Schema, vec![*ty]),
            StdType::Prompt => (BuiltinTypeInfo::Prompt, Vec::new()),
            StdType::PromptPart => (BuiltinTypeInfo::PromptPart, Vec::new()),
            StdType::Message(ty) => (BuiltinTypeInfo::Message, vec![*ty]),
            StdType::MemorySelection(ty) => (BuiltinTypeInfo::MemorySelection, vec![*ty]),
            StdType::Store { key, value } => (BuiltinTypeInfo::Store, vec![*key, *value]),
            StdType::MemoryRegion(ty) => (BuiltinTypeInfo::MemoryRegion, vec![*ty]),
            StdType::ResourceHandleMemoryRegion(ty) => {
                (BuiltinTypeInfo::MemoryRegionHandle, vec![*ty])
            }
            StdType::Trust { wrapper, inner } => (
                match wrapper {
                    StdTrustWrapper::Trusted => BuiltinTypeInfo::Trusted,
                    StdTrustWrapper::Untrusted => BuiltinTypeInfo::Untrusted,
                    StdTrustWrapper::Secret => BuiltinTypeInfo::Secret,
                    StdTrustWrapper::Public => BuiltinTypeInfo::Public,
                    StdTrustWrapper::Sanitized => BuiltinTypeInfo::Sanitized,
                },
                vec![*inner],
            ),
        };
        Ok(TypeInfo::Builtin {
            constructor,
            arguments: sources
                .into_iter()
                .map(|ty| self.standard_ref(ty, scope, module))
                .collect::<Result<_, _>>()?,
        })
    }

    pub(super) fn project_standard_declaration(
        &mut self,
        declaration: &etas_std::TypeDecl,
        identity: &DefinitionKey,
        scope: usize,
    ) -> Result<TypeInfo, ProjectionError> {
        let mut parameters = self.scopes[scope]
            .binders
            .values()
            .copied()
            .collect::<Vec<_>>();
        parameters.sort_by_key(|reference| self.binders[reference.0 as usize].position);
        if declaration.kind == TypeDeclKind::Enum {
            let mut path = identity.module().to_vec();
            path.extend_from_slice(identity.declaration());
            let symbol = self
                .checked
                .std_registry
                .lookup_qualified(&path)
                .ok_or_else(|| ProjectionError::MissingIdentity(path.join(".")))?;
            let constructors = self
                .checked
                .std_registry
                .enum_constructors(symbol.id)
                .cloned()
                .collect::<Vec<_>>();
            if constructors.is_empty() {
                return Err(ProjectionError::Invalid(
                    "std enum declaration has no payload descriptors".into(),
                ));
            }
            let mut variants = Vec::with_capacity(constructors.len());
            for constructor in constructors {
                let StdDecl::Flow(flow) = constructor.decl else {
                    return Err(ProjectionError::Invalid(
                        "std enum constructor has no structured callable declaration".into(),
                    ));
                };
                self.text(&constructor.name)?;
                let types = flow
                    .params
                    .into_iter()
                    .map(|ty| self.standard_ref(ty, scope, identity.module()))
                    .collect::<Result<Vec<_>, _>>()?;
                let payload = if types.is_empty() {
                    VariantPayloadInfo::None
                } else {
                    VariantPayloadInfo::Positional(types)
                };
                variants.push(VariantInfo {
                    name: constructor.name,
                    payload,
                });
            }
            return Ok(TypeInfo::Enum {
                definition: identity.clone(),
                parameters,
                variants,
            });
        }
        if matches!(
            declaration.kind,
            TypeDeclKind::Spec | TypeDeclKind::Primitive
        ) {
            return Err(ProjectionError::Invalid(
                "std spec/primitive declaration was used as a nominal type".into(),
            ));
        }
        Ok(TypeInfo::Nominal {
            definition: identity.clone(),
            parameters,
            representation: declaration
                .representation
                .clone()
                .map(|ty| self.standard_ref(ty, scope, identity.module()))
                .transpose()?,
        })
    }
}

fn primitive_info(primitive: etas_std::StdPrimitiveType) -> PrimitiveInfo {
    use etas_std::StdPrimitiveType as P;
    match primitive {
        P::Unit => PrimitiveInfo::Unit,
        P::Never => PrimitiveInfo::Never,
        P::Bool => PrimitiveInfo::Bool,
        P::I8 => PrimitiveInfo::I8,
        P::I16 => PrimitiveInfo::I16,
        P::I32 => PrimitiveInfo::I32,
        P::I64 => PrimitiveInfo::I64,
        P::I128 => PrimitiveInfo::I128,
        P::ISize => PrimitiveInfo::Isize,
        P::U8 => PrimitiveInfo::U8,
        P::U16 => PrimitiveInfo::U16,
        P::U32 => PrimitiveInfo::U32,
        P::U64 => PrimitiveInfo::U64,
        P::U128 => PrimitiveInfo::U128,
        P::USize => PrimitiveInfo::Usize,
        P::F32 => PrimitiveInfo::F32,
        P::F64 => PrimitiveInfo::F64,
        P::Char => PrimitiveInfo::Char,
        P::String => PrimitiveInfo::String,
        P::Bytes => PrimitiveInfo::Bytes,
    }
}
