use etas_code::descriptor::*;
use etas_types::{ResourceHandleType, TrustWrapper, Type, TypeId};

use super::{DefinitionDescriptorProjector, ProjectionError};
use crate::code::identity::TypeDeclaration;

impl DefinitionDescriptorProjector<'_> {
    pub(super) fn project_type(
        &mut self,
        source: TypeId,
        scope: usize,
    ) -> Result<TypeInfo, ProjectionError> {
        let data = self
            .checked
            .type_store
            .get(source)
            .ok_or(ProjectionError::MissingType(source))?;
        let width = match data {
            Type::Primitive(_)
            | Type::IntegerLiteral { .. }
            | Type::Var(_)
            | Type::Prompt
            | Type::PromptPart => 0,
            Type::Record(record) => record.fields.len(),
            Type::Tuple(elements) => elements.len(),
            Type::Applied { args, .. } => args.len().saturating_add(1),
            Type::Function(flow) => {
                if let Some(row) = &flow.effects {
                    self.ensure_row(row)?;
                }
                flow.input
                    .len()
                    .saturating_add(1)
                    .saturating_add(usize::from(flow.effects.is_some()))
            }
            Type::Handler(handler) => {
                self.ensure_row(&handler.handled)?;
                if let etas_types::HandlerProducedEffects::Explicit(row) = &handler.produced {
                    self.ensure_row(row)?;
                }
                1 + usize::from(handler.result.is_some())
                    + usize::from(matches!(
                        handler.produced,
                        etas_types::HandlerProducedEffects::Explicit(_)
                    ))
            }
            Type::Map { .. } | Type::Result { .. } | Type::Store { .. } => 2,
            _ => 1,
        };
        self.ensure_edges(width)?;
        let data = data.clone();
        let (constructor, arguments) = match data {
            Type::Primitive(primitive) => {
                return Ok(TypeInfo::Primitive(primitive_info(primitive)));
            }
            Type::Named(data) => {
                let binder = self.scopes[scope]
                    .binders
                    .get(&data.name)
                    .copied()
                    .ok_or_else(|| ProjectionError::MissingIdentity(data.name.clone()))?;
                if self.binders[binder.0 as usize].kind != GenericParameterKind::Type {
                    return Err(ProjectionError::UnresolvedType(source));
                }
                return Ok(TypeInfo::Parameter(binder));
            }
            Type::Tuple(elements) => {
                return Ok(TypeInfo::Tuple(
                    elements
                        .into_iter()
                        .map(|ty| self.type_ref(ty, scope))
                        .collect::<Result<_, _>>()?,
                ));
            }
            Type::Record(record) => {
                let mut fields = Vec::with_capacity(record.fields.len());
                for field in record.fields {
                    self.text(&field.name)?;
                    fields.push(FieldInfo {
                        name: field.name,
                        ty: self.type_ref(field.ty, scope)?,
                    });
                }
                return Ok(TypeInfo::Record(fields));
            }
            Type::Applied { constructor, args } => {
                if self.checked.types.known_std_types.code.callable_definition
                    == Some(TypeId(constructor.0))
                {
                    return Ok(TypeInfo::Builtin {
                        constructor: BuiltinTypeInfo::CallableDefinition,
                        arguments: args
                            .into_iter()
                            .map(|ty| self.type_ref(ty, scope))
                            .collect::<Result<_, _>>()?,
                    });
                }
                let constructor = self.type_ref(TypeId(constructor.0), scope)?;
                let arguments = args
                    .into_iter()
                    .map(|ty| self.type_ref(ty, scope))
                    .collect::<Result<_, _>>()?;
                return Ok(TypeInfo::Applied {
                    constructor,
                    arguments,
                });
            }
            Type::Function(flow) => {
                let parameters = flow
                    .input
                    .into_iter()
                    .map(|ty| {
                        Ok(ParameterInfo {
                            name: None,
                            ty: self.type_ref(ty, scope)?,
                        })
                    })
                    .collect::<Result<_, ProjectionError>>()?;
                return Ok(TypeInfo::Function(SignatureInfo {
                    generics: Vec::new(),
                    parameters,
                    output: self.type_ref(flow.output, scope)?,
                    declared_output: None,
                    effects: flow
                        .effects
                        .map(|row| self.row_ref(row, scope))
                        .transpose()?,
                }));
            }
            Type::Handler(handler) => {
                return Ok(TypeInfo::Handler {
                    handled: self.row_ref(handler.handled, scope)?,
                    produced: match handler.produced {
                        etas_types::HandlerProducedEffects::Infer => None,
                        etas_types::HandlerProducedEffects::Explicit(row) => {
                            Some(self.row_ref(row, scope)?)
                        }
                    },
                    result: handler
                        .result
                        .map(|ty| self.type_ref(ty, scope))
                        .transpose()?,
                });
            }
            Type::Array(ty) => (BuiltinTypeInfo::Array, vec![ty]),
            Type::List(ty) => (BuiltinTypeInfo::List, vec![ty]),
            Type::Map { key, value } => (BuiltinTypeInfo::Map, vec![key, value]),
            Type::Set(ty) => (BuiltinTypeInfo::Set, vec![ty]),
            Type::Range { index } => (BuiltinTypeInfo::Range, vec![index]),
            Type::Slice(ty) => (BuiltinTypeInfo::Slice, vec![ty]),
            Type::Option(ty) => (BuiltinTypeInfo::Option, vec![ty]),
            Type::Result { ok, err } => (BuiltinTypeInfo::Result, vec![ok, err]),
            Type::Schema(ty) => (BuiltinTypeInfo::Schema, vec![ty]),
            Type::Prompt => (BuiltinTypeInfo::Prompt, Vec::new()),
            Type::PromptPart => (BuiltinTypeInfo::PromptPart, Vec::new()),
            Type::Message(ty) => (BuiltinTypeInfo::Message, vec![ty]),
            Type::MemorySelection(ty) => (BuiltinTypeInfo::MemorySelection, vec![ty]),
            Type::Store { key, value } => (BuiltinTypeInfo::Store, vec![key, value]),
            Type::MemoryRegion(ty) => (BuiltinTypeInfo::MemoryRegion, vec![ty]),
            Type::ResourceHandle(ResourceHandleType::MemoryRegion { schema }) => {
                (BuiltinTypeInfo::MemoryRegionHandle, vec![schema])
            }
            Type::ResourceHandle(ResourceHandleType::ExternalTool { signature }) => {
                (BuiltinTypeInfo::ExternalToolHandle, vec![signature])
            }
            Type::Trust { wrapper, inner } => (trust_info(wrapper), vec![inner]),
            Type::Nominal(_) | Type::Enum(_) => {
                return Err(ProjectionError::MissingIdentity(format!(
                    "checked type {source:?}"
                )));
            }
            Type::IntegerLiteral { .. }
            | Type::Var(_)
            | Type::Refined { .. }
            | Type::MemoryPlace(_)
            | Type::ResourceHandle(ResourceHandleType::Other { .. }) => {
                return Err(ProjectionError::UnresolvedType(source));
            }
        };
        Ok(TypeInfo::Builtin {
            constructor,
            arguments: arguments
                .into_iter()
                .map(|ty| self.type_ref(ty, scope))
                .collect::<Result<_, _>>()?,
        })
    }

    pub(super) fn project_declaration(
        &mut self,
        source: Option<TypeId>,
        declaration: TypeDeclaration,
        scope: usize,
    ) -> Result<TypeInfo, ProjectionError> {
        if let Some(standard) = declaration.standard {
            let Some(etas_std::StdDecl::Type(standard)) = self
                .checked
                .std_registry
                .symbol(standard)
                .map(|symbol| &symbol.decl)
            else {
                return Err(ProjectionError::Invalid(
                    "std type identity has no canonical declaration".into(),
                ));
            };
            return self.project_standard_declaration(
                &standard.clone(),
                &declaration.identity,
                scope,
            );
        }
        let mut parameters = self.scopes[scope]
            .binders
            .values()
            .copied()
            .collect::<Vec<_>>();
        parameters.sort_by_key(|reference| self.binders[reference.0 as usize].position);
        if let Some(target) = declaration.alias {
            return Ok(TypeInfo::Alias {
                definition: declaration.identity,
                parameters,
                target: self.type_ref(target, scope)?,
            });
        }
        let source = source.ok_or_else(|| {
            ProjectionError::Invalid("source type declaration has no checked type identity".into())
        })?;
        match self
            .checked
            .type_store
            .get(source)
            .ok_or(ProjectionError::MissingType(source))?
            .clone()
        {
            Type::Nominal(nominal) => Ok(TypeInfo::Nominal {
                definition: declaration.identity,
                parameters,
                representation: nominal
                    .representation
                    .map(|ty| self.type_ref(ty, scope))
                    .transpose()?,
            }),
            Type::Enum(_) => {
                let layout = self
                    .checked
                    .types
                    .enum_layouts
                    .get(&source)
                    .ok_or_else(|| {
                        ProjectionError::Invalid(
                            "enum descriptor has no checked payload layout".into(),
                        )
                    })?;
                self.ensure_edges(layout.variants.iter().fold(0usize, |total, variant| {
                    total.saturating_add(variant.fields.len())
                }))?;
                let layout = layout.clone();
                let mut variants = Vec::with_capacity(layout.variants.len());
                for variant in layout.variants {
                    self.text(&variant.name)?;
                    let fields = variant
                        .fields
                        .into_iter()
                        .map(|ty| self.type_ref(ty, scope))
                        .collect::<Result<Vec<_>, _>>()?;
                    let payload = match variant.field_names {
                        Some(names) => {
                            if names.len() != fields.len() {
                                return Err(ProjectionError::Invalid(
                                    "enum payload names and types have different arity".into(),
                                ));
                            }
                            let mut named = Vec::with_capacity(names.len());
                            for (name, ty) in names.into_iter().zip(fields) {
                                self.text(&name)?;
                                named.push(FieldInfo { name, ty });
                            }
                            VariantPayloadInfo::Named(named)
                        }
                        None if fields.is_empty() => VariantPayloadInfo::None,
                        None => VariantPayloadInfo::Positional(fields),
                    };
                    variants.push(VariantInfo {
                        name: variant.name,
                        payload,
                    });
                }
                Ok(TypeInfo::Enum {
                    definition: declaration.identity,
                    parameters,
                    variants,
                })
            }
            _ => Err(ProjectionError::UnresolvedType(source)),
        }
    }
}

pub(super) fn primitive_info(primitive: etas_types::PrimitiveType) -> PrimitiveInfo {
    use etas_types::PrimitiveType as P;
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

pub(super) fn trust_info(wrapper: TrustWrapper) -> BuiltinTypeInfo {
    match wrapper {
        TrustWrapper::Trusted => BuiltinTypeInfo::Trusted,
        TrustWrapper::Untrusted => BuiltinTypeInfo::Untrusted,
        TrustWrapper::Secret => BuiltinTypeInfo::Secret,
        TrustWrapper::Public => BuiltinTypeInfo::Public,
        TrustWrapper::Sanitized => BuiltinTypeInfo::Sanitized,
    }
}
