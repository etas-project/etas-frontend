use std::collections::BTreeMap;

use etas_code::{
    CodeFingerprint,
    descriptor::{
        BuiltinTypeInfo as B, DescriptorError, DescriptorGraph, DescriptorGraphInput,
        DescriptorLimits, FieldInfo, TypeInfo, TypeInfoRef,
    },
    residual::{
        identity::GenericParameterId,
        type_template::{
            StaticTypeNode, StaticTypeTemplate, TemplateField, TypeTemplateError,
            TypeTemplateLimits,
        },
    },
};

use super::{ResourceHandleType, TrustWrapper, Type, TypeId, TypeStore};

impl Type {
    /// The shared descriptor representation of compiler-known type constructors.
    pub fn builtin_descriptor_parts(&self) -> Option<(B, Vec<TypeId>)> {
        Some(match self {
            Self::Array(ty) => (B::Array, vec![*ty]),
            Self::List(ty) => (B::List, vec![*ty]),
            Self::Map { key, value } => (B::Map, vec![*key, *value]),
            Self::Set(ty) => (B::Set, vec![*ty]),
            Self::Range { index } => (B::Range, vec![*index]),
            Self::Slice(ty) => (B::Slice, vec![*ty]),
            Self::Option(ty) => (B::Option, vec![*ty]),
            Self::Result { ok, err } => (B::Result, vec![*ok, *err]),
            Self::Schema(ty) => (B::Schema, vec![*ty]),
            Self::Prompt => (B::Prompt, vec![]),
            Self::PromptPart => (B::PromptPart, vec![]),
            Self::Message(ty) => (B::Message, vec![*ty]),
            Self::MemorySelection(ty) => (B::MemorySelection, vec![*ty]),
            Self::Store { key, value } => (B::Store, vec![*key, *value]),
            Self::MemoryRegion(ty) => (B::MemoryRegion, vec![*ty]),
            Self::ResourceHandle(ResourceHandleType::MemoryRegion { schema }) => {
                (B::MemoryRegionHandle, vec![*schema])
            }
            Self::ResourceHandle(ResourceHandleType::ExternalTool { signature }) => {
                (B::ExternalToolHandle, vec![*signature])
            }
            Self::Trust { wrapper, inner } => (
                match wrapper {
                    TrustWrapper::Trusted => B::Trusted,
                    TrustWrapper::Untrusted => B::Untrusted,
                    TrustWrapper::Secret => B::Secret,
                    TrustWrapper::Public => B::Public,
                    TrustWrapper::Sanitized => B::Sanitized,
                },
                vec![*inner],
            ),
            _ => return None,
        })
    }
}

#[derive(Debug)]
pub enum ClosedStructuralTypeError {
    MissingType(TypeId),
    NodeLimit,
    EdgeLimit,
    TextLimit,
    Cycle,
    InconsistentGraph,
    Descriptor(DescriptorError),
    Template(TypeTemplateError),
}

/// Projects explicit structural types while retaining source-binder identity.
/// Nominal types and unrecognized parameters need a separate identity source.
pub fn symbolic_structural_type_template(
    store: &TypeStore,
    root: TypeId,
    binders: &BTreeMap<etas_hir::SymbolId, GenericParameterId>,
    limits: TypeTemplateLimits,
) -> Result<Option<StaticTypeTemplate>, ClosedStructuralTypeError> {
    let absolute = TypeTemplateLimits::default();
    let max_nodes = limits.max_nodes.min(absolute.max_nodes);
    let max_edges = limits.max_edges.min(absolute.max_edges);
    let max_text = limits.max_text_bytes.min(absolute.max_text_bytes);
    let mut indices = BTreeMap::<TypeId, u32>::new();
    let mut nodes = Vec::new();
    let mut states = BTreeMap::<TypeId, u8>::new();
    let mut work = vec![(root, false)];
    let mut edges = 0usize;
    let mut text = 0usize;
    let mut discovered = 0usize;
    while let Some((id, finish)) = work.pop() {
        let ty = store
            .get(id)
            .ok_or(ClosedStructuralTypeError::MissingType(id))?;
        if finish {
            let reference = |id: &TypeId| {
                indices
                    .get(id)
                    .copied()
                    .ok_or(ClosedStructuralTypeError::InconsistentGraph)
            };
            let node = match ty {
                Type::Primitive(primitive) => {
                    StaticTypeNode::Primitive(primitive.descriptor_info())
                }
                Type::Named(named) => {
                    let Some(parameter) = named
                        .source_parameter
                        .and_then(|symbol| binders.get(&symbol))
                        .copied()
                    else {
                        return Ok(None);
                    };
                    StaticTypeNode::Parameter(parameter)
                }
                Type::Tuple(elems) => {
                    StaticTypeNode::Tuple(elems.iter().map(reference).collect::<Result<_, _>>()?)
                }
                Type::Record(record) => StaticTypeNode::Record(
                    record
                        .fields
                        .iter()
                        .map(|field| {
                            Ok(TemplateField {
                                name: field.name.clone(),
                                ty: reference(&field.ty)?,
                            })
                        })
                        .collect::<Result<_, ClosedStructuralTypeError>>()?,
                ),
                _ => {
                    let Some((constructor, args)) = ty.builtin_descriptor_parts() else {
                        return Ok(None);
                    };
                    StaticTypeNode::Builtin {
                        constructor,
                        arguments: args.iter().map(reference).collect::<Result<_, _>>()?,
                    }
                }
            };
            if nodes.len() >= max_nodes {
                return Err(ClosedStructuralTypeError::NodeLimit);
            }
            let index =
                u32::try_from(nodes.len()).map_err(|_| ClosedStructuralTypeError::NodeLimit)?;
            nodes.push(node);
            indices.insert(id, index);
            states.insert(id, 2);
            continue;
        }
        match states.get(&id) {
            Some(1) => return Err(ClosedStructuralTypeError::Cycle),
            Some(2) => continue,
            _ => {}
        }
        discovered = discovered
            .checked_add(1)
            .ok_or(ClosedStructuralTypeError::NodeLimit)?;
        if discovered > max_nodes {
            return Err(ClosedStructuralTypeError::NodeLimit);
        }
        let width = match ty {
            Type::Primitive(_) | Type::Named(_) => 0,
            Type::Tuple(elems) => elems.len(),
            Type::Record(record) => record.fields.len(),
            _ => match ty.builtin_descriptor_parts() {
                Some((_, args)) => args.len(),
                None => return Ok(None),
            },
        };
        edges = edges
            .checked_add(width)
            .ok_or(ClosedStructuralTypeError::EdgeLimit)?;
        if edges > max_edges {
            return Err(ClosedStructuralTypeError::EdgeLimit);
        }
        if let Type::Record(record) = ty {
            for field in &record.fields {
                text = text
                    .checked_add(field.name.len())
                    .ok_or(ClosedStructuralTypeError::TextLimit)?;
                if text > max_text {
                    return Err(ClosedStructuralTypeError::TextLimit);
                }
            }
        }
        let children = match ty {
            Type::Primitive(_) | Type::Named(_) => vec![],
            Type::Tuple(elems) => elems.clone(),
            Type::Record(record) => record.fields.iter().map(|field| field.ty).collect(),
            _ => match ty.builtin_descriptor_parts() {
                Some((_, args)) => args,
                None => return Ok(None),
            },
        };
        states.insert(id, 1);
        work.push((id, true));
        for child in children.into_iter().rev() {
            work.push((child, false));
        }
    }
    let root = indices
        .get(&root)
        .copied()
        .ok_or(ClosedStructuralTypeError::InconsistentGraph)?;
    StaticTypeTemplate::new(nodes, root, limits)
        .map(Some)
        .map_err(ClosedStructuralTypeError::Template)
}

/// Keeps a checked source binder symbolic until its callable application is
/// known. Unsupported nominal graphs and unknown binders are not guessed.
pub fn symbolic_action_type_selector(
    store: &TypeStore,
    root: TypeId,
    binders: &BTreeMap<etas_hir::SymbolId, GenericParameterId>,
    limits: TypeTemplateLimits,
) -> Result<Option<etas_code::residual::pattern::ActionSelectorTemplate>, ClosedStructuralTypeError>
{
    use etas_code::residual::pattern::ActionSelectorTemplate;
    let Some(template) = symbolic_structural_type_template(store, root, binders, limits)? else {
        return Ok(None);
    };
    if let [StaticTypeNode::Parameter(parameter)] = template.nodes() {
        Ok(Some(ActionSelectorTemplate::TypeParameter(*parameter)))
    } else {
        Ok(Some(ActionSelectorTemplate::TypeTemplate(template)))
    }
}

/// Projects only fully closed structural/builtin types. Named declarations,
/// symbolic binders, effectful signatures and unsupported nodes are not
/// guessed; their stable declaration environment must be supplied elsewhere.
pub fn closed_structural_type_fingerprint(
    store: &TypeStore,
    root: TypeId,
    limits: DescriptorLimits,
) -> Result<Option<CodeFingerprint>, ClosedStructuralTypeError> {
    let mut indices = BTreeMap::new();
    let mut nodes: Vec<Option<TypeInfo>> = Vec::new();
    let mut states = BTreeMap::<TypeId, u8>::new();
    let mut work = vec![(root, false)];
    let mut edges = 0usize;
    let mut text_bytes = 0usize;
    while let Some((id, finish)) = work.pop() {
        let ty = store
            .get(id)
            .ok_or(ClosedStructuralTypeError::MissingType(id))?;
        if finish {
            let reference = |id: &TypeId| {
                indices
                    .get(id)
                    .copied()
                    .ok_or(ClosedStructuralTypeError::InconsistentGraph)
            };
            let node = match ty {
                Type::Primitive(primitive) => TypeInfo::Primitive(primitive.descriptor_info()),
                Type::Tuple(elems) => {
                    TypeInfo::Tuple(elems.iter().map(reference).collect::<Result<_, _>>()?)
                }
                Type::Record(record) => TypeInfo::Record(
                    record
                        .fields
                        .iter()
                        .map(|field| {
                            Ok(FieldInfo {
                                name: field.name.clone(),
                                ty: reference(&field.ty)?,
                            })
                        })
                        .collect::<Result<_, ClosedStructuralTypeError>>()?,
                ),
                _ => {
                    let Some((constructor, args)) = ty.builtin_descriptor_parts() else {
                        return Ok(None);
                    };
                    TypeInfo::Builtin {
                        constructor,
                        arguments: args.iter().map(reference).collect::<Result<_, _>>()?,
                    }
                }
            };
            let index = reference(&id)?.0 as usize;
            *nodes
                .get_mut(index)
                .ok_or(ClosedStructuralTypeError::InconsistentGraph)? = Some(node);
            states.insert(id, 2);
            continue;
        }
        match states.get(&id) {
            Some(1) => return Err(ClosedStructuralTypeError::Cycle),
            Some(2) => continue,
            _ => {}
        }
        if let std::collections::btree_map::Entry::Vacant(entry) = indices.entry(id) {
            if nodes.len() >= limits.max_nodes || nodes.len() >= u32::MAX as usize {
                return Err(ClosedStructuralTypeError::NodeLimit);
            }
            entry.insert(TypeInfoRef(nodes.len() as u32));
            nodes.push(None);
        }
        let width = match ty {
            Type::Primitive(_) => 0,
            Type::Tuple(elems) => elems.len(),
            Type::Record(record) => record.fields.len(),
            _ => match ty.builtin_descriptor_parts() {
                Some((_, args)) => args.len(),
                None => return Ok(None),
            },
        };
        edges = edges
            .checked_add(width)
            .ok_or(ClosedStructuralTypeError::EdgeLimit)?;
        if edges > limits.max_edges {
            return Err(ClosedStructuralTypeError::EdgeLimit);
        }
        if let Type::Record(record) = ty {
            for field in &record.fields {
                text_bytes = text_bytes
                    .checked_add(field.name.len())
                    .ok_or(ClosedStructuralTypeError::TextLimit)?;
                if text_bytes > limits.max_text_bytes {
                    return Err(ClosedStructuralTypeError::TextLimit);
                }
            }
        }
        let children: Vec<TypeId> = match ty {
            Type::Primitive(_) => vec![],
            Type::Tuple(elems) => elems.clone(),
            Type::Record(record) => record.fields.iter().map(|field| field.ty).collect(),
            _ => match ty.builtin_descriptor_parts() {
                Some((_, args)) => args,
                None => return Ok(None),
            },
        };
        states.insert(id, 1);
        work.push((id, true));
        for child in children.into_iter().rev() {
            work.push((child, false));
        }
    }
    let root = indices
        .get(&root)
        .copied()
        .ok_or(ClosedStructuralTypeError::InconsistentGraph)?;
    let input = DescriptorGraphInput {
        types: nodes
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or(ClosedStructuralTypeError::InconsistentGraph)?,
        ..Default::default()
    };
    let graph =
        DescriptorGraph::new(input, limits).map_err(ClosedStructuralTypeError::Descriptor)?;
    graph
        .type_fingerprint(root)
        .map(Some)
        .map_err(ClosedStructuralTypeError::Descriptor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FieldType, NamedTypeRef, NominalTypeRef, PrimitiveType, RecordType, TypeInterner};
    use etas_code::descriptor::PrimitiveInfo;

    #[test]
    fn closed_type_identity_matches_descriptor_graph_and_ignores_interner_order() {
        let mut first = TypeInterner::new();
        let i32_type = first.primitive(PrimitiveType::I32);
        let array = first.intern(Type::Array(i32_type));
        let actual =
            closed_structural_type_fingerprint(first.store(), array, DescriptorLimits::default())
                .unwrap()
                .unwrap();
        let graph = DescriptorGraph::new(
            DescriptorGraphInput {
                types: vec![
                    TypeInfo::Builtin {
                        constructor: B::Array,
                        arguments: vec![TypeInfoRef(1)],
                    },
                    TypeInfo::Primitive(PrimitiveInfo::I32),
                ],
                ..Default::default()
            },
            DescriptorLimits::default(),
        )
        .unwrap();
        assert_eq!(actual, graph.type_fingerprint(TypeInfoRef(0)).unwrap());
        let mut second = TypeInterner::new();
        second.primitive(PrimitiveType::String);
        let i32_type = second.primitive(PrimitiveType::I32);
        let array = second.intern(Type::Array(i32_type));
        assert_eq!(
            actual,
            closed_structural_type_fingerprint(second.store(), array, DescriptorLimits::default())
                .unwrap()
                .unwrap()
        );
        let tuple = second.intern(Type::Tuple(vec![array, array]));
        assert!(
            closed_structural_type_fingerprint(second.store(), tuple, DescriptorLimits::default())
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn closed_type_identity_rejects_unresolved_and_enforces_limits() {
        let mut interner = TypeInterner::new();
        let named = interner.intern(Type::Named(NamedTypeRef {
            name: "T".into(),
            source_parameter: None,
        }));
        let generic_array = interner.intern(Type::Array(named));
        assert!(
            closed_structural_type_fingerprint(
                interner.store(),
                generic_array,
                DescriptorLimits::default()
            )
            .unwrap()
            .is_none()
        );
        let integer = interner.primitive(PrimitiveType::I32);
        let nominal = interner.intern(Type::Nominal(NominalTypeRef {
            name: "app.UserId".into(),
            params: vec![],
            representation: Some(integer),
            read_only_erased_view: false,
        }));
        assert!(
            closed_structural_type_fingerprint(
                interner.store(),
                nominal,
                DescriptorLimits::default()
            )
            .unwrap()
            .is_none()
        );
        let array = interner.intern(Type::Array(integer));
        let limits = DescriptorLimits {
            max_nodes: 1,
            ..DescriptorLimits::default()
        };
        assert!(matches!(
            closed_structural_type_fingerprint(interner.store(), array, limits),
            Err(ClosedStructuralTypeError::NodeLimit)
        ));
        let limits = DescriptorLimits {
            max_edges: 0,
            ..DescriptorLimits::default()
        };
        assert!(matches!(
            closed_structural_type_fingerprint(interner.store(), array, limits),
            Err(ClosedStructuralTypeError::EdgeLimit)
        ));
        let record = interner.intern(Type::Record(RecordType {
            fields: vec![FieldType {
                name: "lengthy".into(),
                ty: integer,
            }],
        }));
        let limits = DescriptorLimits {
            max_text_bytes: 2,
            ..DescriptorLimits::default()
        };
        assert!(matches!(
            closed_structural_type_fingerprint(interner.store(), record, limits),
            Err(ClosedStructuralTypeError::TextLimit)
        ));
        assert!(matches!(
            closed_structural_type_fingerprint(
                interner.store(),
                TypeId(u32::MAX),
                DescriptorLimits::default()
            ),
            Err(ClosedStructuralTypeError::MissingType(_))
        ));
    }
}
