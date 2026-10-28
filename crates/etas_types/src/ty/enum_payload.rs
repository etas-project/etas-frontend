use std::collections::{HashMap, HashSet};

use etas_utils::{GraphView, strongly_connected_components};

use super::{
    HandlerProducedEffects, ResourceHandleType, Type, TypeId, TypeInterner, TypeStore,
    TypeSubstitutionEngine, TypeSubstitutionError,
};
use crate::EnumLayoutFact;

#[cfg(test)]
mod tests;

pub(super) struct AcyclicEnumPayloads<'a> {
    layouts: &'a HashMap<TypeId, EnumLayoutFact>,
    recursive: HashSet<TypeId>,
}

pub(super) enum EnumPayloadExpansion {
    GenericDeclaration,
    RecursiveTemplate,
    Concrete(Vec<TypeId>),
}

impl<'a> AcyclicEnumPayloads<'a> {
    pub(super) fn new(
        store: &TypeStore,
        layouts: &'a HashMap<TypeId, EnumLayoutFact>,
    ) -> Result<Self, TypeSubstitutionError> {
        let mut graph = PayloadGraph::default();
        for owner in layouts.keys() {
            if !matches!(store.get(*owner), Some(Type::Enum(_))) {
                return Err(TypeSubstitutionError::InvalidApplication(*owner));
            }
        }
        let mut pending = layouts.keys().copied().collect::<Vec<_>>();
        while let Some(ty) = pending.pop() {
            if graph.edges.contains_key(&ty) {
                continue;
            }
            if graph.edges.len() >= super::representation::MAX_REPRESENTATION_TYPES {
                return Err(TypeSubstitutionError::RepresentationLimit(
                    super::representation::MAX_REPRESENTATION_TYPES,
                ));
            }
            let data = store
                .get(ty)
                .ok_or(TypeSubstitutionError::MissingType(ty))?;
            let mut dependencies = Vec::new();
            match data {
                Type::Enum(_) => {
                    let layout = layouts
                        .get(&ty)
                        .ok_or(TypeSubstitutionError::MissingEnumLayout(ty))?;
                    dependencies.extend(
                        layout
                            .variants
                            .iter()
                            .flat_map(|variant| variant.fields.iter().copied()),
                    );
                }
                Type::Applied { constructor, args } => {
                    dependencies.push(TypeId(constructor.0));
                    dependencies.extend(args.iter().copied());
                }
                Type::Nominal(nominal) => dependencies.extend(nominal.representation),
                _ => enqueue_structural_children(data, &mut dependencies),
            }
            dependencies.sort_unstable();
            dependencies.dedup();
            pending.extend(dependencies.iter().copied());
            graph.edges.insert(ty, dependencies);
        }
        let recursive = strongly_connected_components(&graph)
            .into_iter()
            .filter(|component| {
                component.nodes.len() > 1
                    || component
                        .nodes
                        .first()
                        .is_some_and(|node| graph.successors(node).contains(node))
            })
            .flat_map(|component| component.nodes)
            .filter(|ty| layouts.contains_key(ty))
            .collect();
        Ok(Self { layouts, recursive })
    }

    pub(super) fn expand(
        &self,
        interner: &mut TypeInterner,
        owner: TypeId,
        args: &[TypeId],
        application: bool,
    ) -> Result<EnumPayloadExpansion, TypeSubstitutionError> {
        let layout = self
            .layouts
            .get(&owner)
            .ok_or(TypeSubstitutionError::MissingEnumLayout(owner))?;
        if !application && !layout.type_params.is_empty() {
            return Ok(EnumPayloadExpansion::GenericDeclaration);
        }
        if args.len() != layout.type_params.len() {
            return Err(TypeSubstitutionError::InvalidApplication(owner));
        }
        // Recursive declarations retain their checked templates. Expanding an
        // application here would enumerate Growing<T>, Growing<List<T>>, ... .
        if self.recursive.contains(&owner) {
            return Ok(EnumPayloadExpansion::RecursiveTemplate);
        }
        let bindings = layout
            .type_params
            .iter()
            .cloned()
            .zip(args.iter().copied())
            .collect();
        let mut substitution = TypeSubstitutionEngine::materializing(interner, &bindings, None);
        layout
            .variants
            .iter()
            .flat_map(|variant| &variant.fields)
            .map(|ty| substitution.substitute(*ty))
            .collect::<Result<Vec<_>, _>>()
            .map(EnumPayloadExpansion::Concrete)
    }
}

#[derive(Default)]
struct PayloadGraph {
    edges: HashMap<TypeId, Vec<TypeId>>,
}

impl GraphView for PayloadGraph {
    type Node = TypeId;

    fn nodes(&self) -> Vec<TypeId> {
        let mut nodes = self.edges.keys().copied().collect::<Vec<_>>();
        nodes.sort_unstable();
        nodes
    }

    fn successors(&self, node: &TypeId) -> Vec<TypeId> {
        self.edges.get(node).cloned().unwrap_or_default()
    }
}

fn enqueue_structural_children(ty: &Type, pending: &mut Vec<TypeId>) {
    match ty {
        Type::Array(inner)
        | Type::List(inner)
        | Type::Set(inner)
        | Type::Slice(inner)
        | Type::Option(inner)
        | Type::Range { index: inner }
        | Type::Message(inner)
        | Type::Schema(inner)
        | Type::MemorySelection(inner)
        | Type::MemoryRegion(inner)
        | Type::Trust { inner, .. }
        | Type::Refined { base: inner, .. } => pending.push(*inner),
        Type::Map { key, value }
        | Type::Store { key, value }
        | Type::Result {
            ok: key,
            err: value,
        } => pending.extend([*key, *value]),
        Type::Tuple(types) => pending.extend(types.iter().copied()),
        Type::Record(record) => pending.extend(record.fields.iter().map(|field| field.ty)),
        Type::Function(flow) => {
            pending.extend(flow.input.iter().copied());
            pending.push(flow.output);
            if let Some(row) = &flow.effects {
                enqueue_row(row, pending);
            }
        }
        Type::Handler(handler) => {
            pending.extend(handler.result);
            enqueue_row(&handler.handled, pending);
            if let HandlerProducedEffects::Explicit(row) = &handler.produced {
                enqueue_row(row, pending);
            }
        }
        Type::ResourceHandle(ResourceHandleType::MemoryRegion { schema }) => pending.push(*schema),
        Type::ResourceHandle(ResourceHandleType::ExternalTool { signature }) => {
            pending.push(*signature)
        }
        Type::ResourceHandle(ResourceHandleType::Other { args, .. }) => {
            pending.extend(args.iter().copied())
        }
        _ => {}
    }
}

fn enqueue_row(row: &super::EffectRowRef, pending: &mut Vec<TypeId>) {
    pending.extend(
        row.effects
            .iter()
            .flat_map(|effect| &effect.args)
            .filter_map(|arg| match arg {
                super::EffectArgRef::Type(ty) => Some(*ty),
                _ => None,
            }),
    );
}
