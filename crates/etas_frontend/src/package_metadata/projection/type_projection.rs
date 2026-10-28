use super::*;
use std::collections::BTreeSet;

#[cfg(test)]
mod tests;

pub(super) fn metadata_type(
    ty: TypeId,
    store: &TypeStore,
) -> Result<MetadataType, PackageMetadataError> {
    TypeProjection::new(store).ty(ty)
}

pub(super) fn metadata_effect_row_ref(
    row: &EffectRowRef,
    store: &TypeStore,
) -> Result<MetadataEffectRow, PackageMetadataError> {
    TypeProjection::new(store).row(row)
}

pub(super) fn metadata_effect_arg(
    arg: &EffectArgRef,
    store: &TypeStore,
) -> Result<MetadataEffectArg, PackageMetadataError> {
    TypeProjection::new(store).arg(arg)
}

/// Nominal recursion is encoded through canonical references, not repeated
/// representations. Structural cycles still require a nominal boundary.
struct TypeProjection<'a> {
    store: &'a TypeStore,
    nominals: BTreeSet<TypeId>,
    structural: BTreeSet<(TypeId, Option<TypeId>)>,
    anchor: Option<TypeId>,
    depth: usize,
    nodes: usize,
}

impl<'a> TypeProjection<'a> {
    fn new(store: &'a TypeStore) -> Self {
        Self {
            store,
            nominals: BTreeSet::new(),
            structural: BTreeSet::new(),
            anchor: None,
            depth: 0,
            nodes: 0,
        }
    }

    fn ty(&mut self, ty: TypeId) -> Result<MetadataType, PackageMetadataError> {
        let store = self.store;
        let Some(ty_value) = store.get(ty) else {
            return Err(PackageMetadataError::UnsupportedType {
                ty,
                reason: "type id is missing from type store".to_owned(),
            });
        };
        self.nodes += 1;
        if self.nodes > etas_package_metadata::MAX_METADATA_GRAPH_NODES
            || self.depth > etas_package_metadata::MAX_METADATA_GRAPH_DEPTH
        {
            return Err(PackageMetadataError::UnsupportedType {
                ty,
                reason: "package type projection exceeds the metadata graph budget".into(),
            });
        }
        let parent = self.anchor;
        let key = (ty, parent);
        if let Type::Nominal(nominal) = ty_value {
            if !self.nominals.insert(ty) {
                return Ok(MetadataType {
                    kind: MetadataTypeKind::Named,
                    path: nominal.name.split('.').map(str::to_owned).collect(),
                    ..Default::default()
                });
            }
            self.anchor = Some(ty);
        } else if !self.structural.insert(key) {
            return Err(PackageMetadataError::UnsupportedType {
                ty,
                reason: "recursive structural type has no nominal identity boundary".into(),
            });
        }
        self.depth += 1;
        let mut metadata = MetadataType::default();
        match ty_value {
            Type::Primitive(primitive) => {
                metadata.kind = MetadataTypeKind::Primitive;
                metadata.name = primitive_name(*primitive).to_owned();
            }
            Type::IntegerLiteral { text } => {
                metadata.kind = MetadataTypeKind::Primitive;
                metadata.name = text.clone();
            }
            Type::Var(var) => {
                metadata.kind = MetadataTypeKind::Var;
                metadata.name = format!("T{}", var.0);
            }
            Type::Array(element) => {
                self.unary_type(MetadataTypeKind::Array, *element, &mut metadata)?
            }
            Type::List(element) => {
                self.unary_type(MetadataTypeKind::List, *element, &mut metadata)?
            }
            Type::Map { key, value } => {
                metadata.kind = MetadataTypeKind::Map;
                metadata.children.push(self.ty(*key)?);
                metadata.children.push(self.ty(*value)?);
            }
            Type::Set(element) => {
                self.unary_type(MetadataTypeKind::Set, *element, &mut metadata)?
            }
            Type::Range { index } => {
                self.unary_type(MetadataTypeKind::Range, *index, &mut metadata)?
            }
            Type::Slice(element) => {
                self.unary_type(MetadataTypeKind::Slice, *element, &mut metadata)?
            }
            Type::Option(inner) => {
                self.unary_type(MetadataTypeKind::Option, *inner, &mut metadata)?
            }
            Type::Result { ok, err } => {
                metadata.kind = MetadataTypeKind::Result;
                metadata.children.push(self.ty(*ok)?);
                metadata.children.push(self.ty(*err)?);
            }
            Type::Tuple(elements) => {
                metadata.kind = MetadataTypeKind::Tuple;
                metadata.children = elements
                    .iter()
                    .map(|element| self.ty(*element))
                    .collect::<Result<Vec<_>, _>>()?;
            }
            Type::Function(flow) => {
                metadata.kind = MetadataTypeKind::Function;
                metadata.children = flow
                    .input
                    .iter()
                    .map(|input| self.ty(*input))
                    .collect::<Result<Vec<_>, _>>()?;
                metadata.children.push(self.ty(flow.output)?);
                metadata.effects = flow.effects.as_ref().map(|row| self.row(row)).transpose()?;
            }
            Type::Handler(handler) => {
                metadata.kind = MetadataTypeKind::Handler;
                metadata.effects = Some(self.row(&handler.handled)?);
                metadata.produced_effects = match &handler.produced {
                    etas_types::HandlerProducedEffects::Infer => None,
                    etas_types::HandlerProducedEffects::Explicit(row) => Some(self.row(row)?),
                };
                if let Some(result) = handler.result {
                    metadata.children.push(self.ty(result)?);
                }
            }
            Type::Enum(enum_ref) => {
                metadata.kind = MetadataTypeKind::Named;
                metadata.path = enum_ref.name.split('.').map(str::to_owned).collect();
            }
            Type::Named(named) => {
                metadata.kind = MetadataTypeKind::Named;
                metadata.path = named.name.split('.').map(str::to_owned).collect();
            }
            Type::Nominal(nominal) => {
                metadata.kind = MetadataTypeKind::Nominal;
                metadata.path = nominal.name.split('.').map(str::to_owned).collect();
                if let Some(representation) = nominal.representation {
                    metadata.children.push(self.ty(representation)?);
                }
            }
            Type::Applied { constructor, args } => {
                metadata.kind = MetadataTypeKind::Applied;
                metadata.path = applied_type_constructor_path(*constructor, store)?;
                metadata.children = args
                    .iter()
                    .map(|arg| self.ty(*arg))
                    .collect::<Result<Vec<_>, _>>()?;
            }
            Type::Trust { wrapper, inner } => {
                metadata.kind = trust_wrapper_kind(*wrapper);
                metadata.children.push(self.ty(*inner)?);
            }
            Type::Prompt => metadata.kind = MetadataTypeKind::Prompt,
            Type::PromptPart => metadata.kind = MetadataTypeKind::PromptPart,
            Type::Message(inner) => {
                self.unary_type(MetadataTypeKind::Message, *inner, &mut metadata)?
            }
            Type::MemorySelection(inner) => {
                self.unary_type(MetadataTypeKind::MemorySelection, *inner, &mut metadata)?
            }
            Type::Store { key, value } => {
                metadata.kind = MetadataTypeKind::Store;
                metadata.children.push(self.ty(*key)?);
                metadata.children.push(self.ty(*value)?);
            }
            Type::MemoryRegion(schema) => {
                self.unary_type(MetadataTypeKind::MemoryRegion, *schema, &mut metadata)?
            }
            Type::ResourceHandle(handle) => {
                metadata.kind = MetadataTypeKind::ResourceHandle;
                match handle {
                    ResourceHandleType::MemoryRegion { schema } => {
                        metadata.name = "MemoryRegion".to_owned();
                        metadata.children.push(self.ty(*schema)?);
                    }
                    ResourceHandleType::ExternalTool { signature } => {
                        metadata.name = "ExternalTool".to_owned();
                        metadata.children.push(self.ty(*signature)?);
                    }
                    ResourceHandleType::Other { name, args } => {
                        metadata.name = name.clone();
                        metadata.children =
                            args.iter()
                                .map(|arg| self.ty(*arg))
                                .collect::<Result<Vec<_>, _>>()?;
                    }
                }
            }
            Type::Record(record) => {
                metadata.kind = MetadataTypeKind::Record;
                metadata.fields = record
                    .fields
                    .iter()
                    .map(|field| {
                        Ok(MetadataTypeField {
                            name: field.name.clone(),
                            ty: self.ty(field.ty)?,
                        })
                    })
                    .collect::<Result<Vec<_>, PackageMetadataError>>()?;
            }
            Type::Refined { .. } | Type::Schema(_) | Type::MemoryPlace(_) => {
                return Err(PackageMetadataError::UnsupportedType {
                    ty,
                    reason: format!("{ty_value:?} has no published metadata schema yet"),
                });
            }
        }
        self.depth -= 1;
        self.anchor = parent;
        if matches!(ty_value, Type::Nominal(_)) {
            self.nominals.remove(&ty);
        } else {
            self.structural.remove(&key);
        }
        Ok(metadata)
    }

    fn unary_type(
        &mut self,
        kind: MetadataTypeKind,
        inner: TypeId,
        metadata: &mut MetadataType,
    ) -> Result<(), PackageMetadataError> {
        metadata.kind = kind;
        metadata.children.push(self.ty(inner)?);
        Ok(())
    }

    fn row(&mut self, row: &EffectRowRef) -> Result<MetadataEffectRow, PackageMetadataError> {
        Ok(MetadataEffectRow {
            effects: row
                .effects
                .iter()
                .map(|effect| self.effect(effect))
                .collect::<Result<Vec<_>, _>>()?,
            tail: row.tail.clone(),
        })
    }

    fn effect(&mut self, effect: &EffectRef) -> Result<MetadataEffectRef, PackageMetadataError> {
        Ok(MetadataEffectRef {
            path: effect.name.split('.').map(str::to_owned).collect(),
            args: effect
                .args
                .iter()
                .map(|arg| self.arg(arg))
                .collect::<Result<Vec<_>, _>>()?,
        })
    }

    fn arg(&mut self, arg: &EffectArgRef) -> Result<MetadataEffectArg, PackageMetadataError> {
        match arg {
            EffectArgRef::Type(ty) => Ok(MetadataEffectArg {
                kind: MetadataEffectArgKind::Type,
                ty: Some(self.ty(*ty)?),
                path: Vec::new(),
                value: String::new(),
            }),
            EffectArgRef::Wildcard => Ok(MetadataEffectArg {
                kind: MetadataEffectArgKind::Wildcard,
                ty: None,
                path: Vec::new(),
                value: String::new(),
            }),
            EffectArgRef::String(value) => Ok(MetadataEffectArg {
                kind: MetadataEffectArgKind::String,
                ty: None,
                path: Vec::new(),
                value: value.clone(),
            }),
            EffectArgRef::Int(value) => Ok(MetadataEffectArg {
                kind: MetadataEffectArgKind::Int,
                ty: None,
                path: Vec::new(),
                value: value.clone(),
            }),
            EffectArgRef::Path(path) => Ok(MetadataEffectArg {
                kind: MetadataEffectArgKind::Path,
                ty: None,
                path: path.clone(),
                value: String::new(),
            }),
        }
    }
}
