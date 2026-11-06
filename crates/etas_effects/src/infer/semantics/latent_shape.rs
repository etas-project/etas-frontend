use std::collections::{BTreeSet, VecDeque};

use etas_types::{Type, TypeId, TypeOutput};

#[derive(Clone, Default, PartialEq, Eq)]
struct Shape {
    callable: bool,
    parameters: BTreeSet<TypeId>,
}

impl Shape {
    fn union(&mut self, other: &Self) {
        self.callable |= other.callable;
        self.parameters.extend(&other.parameters);
    }
}

pub(crate) struct LatentTypeShapes {
    shapes: Vec<Shape>,
    opaque_parameters: BTreeSet<TypeId>,
}

impl LatentTypeShapes {
    pub(super) fn build(types: &TypeOutput, descriptor: impl Fn(TypeId) -> bool) -> Self {
        let mut result = Self {
            shapes: vec![Shape::default(); types.store.len()],
            opaque_parameters: types
                .store
                .iter()
                .filter_map(|(ty, value)| match value {
                    Type::Named(named)
                        if named.source_parameter.is_some_and(|symbol| {
                            types
                                .facts
                                .type_param_bounds
                                .get(&symbol)
                                .is_some_and(|bounds| !bounds.is_empty())
                        }) =>
                    {
                        Some(ty)
                    }
                    _ => None,
                })
                .collect(),
        };
        let mut parents = vec![Vec::new(); types.store.len()];
        for (ty, value) in types.store.iter() {
            for child in children(types, ty, value) {
                parents[child.0 as usize].push(ty);
            }
        }
        let mut pending = types
            .store
            .iter()
            .map(|(ty, _)| ty)
            .collect::<VecDeque<_>>();
        let mut queued = vec![true; types.store.len()];
        // Summarize payload parameter demand instead of expanding recursive
        // applications, whose concrete arguments may grow without a fixed depth.
        while let Some(ty) = pending.pop_front() {
            queued[ty.0 as usize] = false;
            let shape = result.transfer(types, ty, descriptor(ty));
            if result.shapes[ty.0 as usize] == shape {
                continue;
            }
            result.shapes[ty.0 as usize] = shape;
            for parent in &parents[ty.0 as usize] {
                if !queued[parent.0 as usize] {
                    queued[parent.0 as usize] = true;
                    pending.push_back(*parent);
                }
            }
        }
        result
    }

    pub(super) fn contains(&self, ty: TypeId) -> bool {
        let shape = &self.shapes[ty.0 as usize];
        shape.callable
            || shape
                .parameters
                .iter()
                .any(|ty| self.opaque_parameters.contains(ty))
    }

    fn transfer(&self, types: &TypeOutput, ty: TypeId, descriptor: bool) -> Shape {
        let value = types.store.get(ty).expect("indexed checked type shape");
        let mut shape = Shape::default();
        if descriptor || matches!(value, Type::Function(_) | Type::Existential { .. }) {
            shape.callable = true;
        } else if matches!(value, Type::Named(_)) {
            shape.parameters.insert(ty);
        } else if let Type::Applied { constructor, args } = value {
            let owner = TypeId(constructor.0);
            let Some(parameters) = types.facts.type_generics.get(&owner) else {
                shape.callable = true;
                return shape;
            };
            if parameters.len() != args.len() {
                shape.callable = true;
                return shape;
            }
            let template = &self.shapes[owner.0 as usize];
            shape.callable = template.callable;
            for parameter in &template.parameters {
                if let Some(position) = parameters
                    .iter()
                    .position(|binding| binding.subject == *parameter)
                {
                    shape.union(&self.shapes[args[position].0 as usize]);
                } else {
                    shape.parameters.insert(*parameter);
                }
            }
        } else if matches!(value, Type::Enum(_)) && !types.facts.enum_layouts.contains_key(&ty) {
            shape.callable = true;
        } else {
            for child in children(types, ty, value) {
                shape.union(&self.shapes[child.0 as usize]);
            }
        }
        shape
    }
}

fn children(types: &TypeOutput, ty: TypeId, value: &Type) -> Vec<TypeId> {
    match value {
        Type::Array(inner)
        | Type::List(inner)
        | Type::Set(inner)
        | Type::Slice(inner)
        | Type::Option(inner)
        | Type::Schema(inner)
        | Type::Message(inner)
        | Type::MemorySelection(inner)
        | Type::MemoryRegion(inner)
        | Type::Refined { base: inner, .. }
        | Type::Trust { inner, .. } => vec![*inner],
        Type::Map { key, value }
        | Type::Store { key, value }
        | Type::Result {
            ok: key,
            err: value,
        } => vec![*key, *value],
        Type::Record(record) => record.fields.iter().map(|field| field.ty).collect(),
        Type::Tuple(elements) => elements.clone(),
        Type::Nominal(nominal) => nominal.representation.into_iter().collect(),
        Type::Applied { constructor, args } => std::iter::once(TypeId(constructor.0))
            .chain(args.iter().copied())
            .collect(),
        Type::Enum(_) => types
            .facts
            .enum_layouts
            .get(&ty)
            .into_iter()
            .flat_map(|layout| &layout.variants)
            .flat_map(|variant| variant.fields.iter().copied())
            .collect(),
        Type::Function(_)
        | Type::Existential { .. }
        | Type::Primitive(_)
        | Type::IntegerLiteral { .. }
        | Type::Var(_)
        | Type::Range { .. }
        | Type::Handler(_)
        | Type::Task(_)
        | Type::SchedulerHandle
        | Type::Continuation(_)
        | Type::Named(_)
        | Type::Prompt
        | Type::PromptPart
        | Type::MemoryPlace(_)
        | Type::ResourceHandle(_) => Vec::new(),
    }
}
