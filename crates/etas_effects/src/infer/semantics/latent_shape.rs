use std::collections::{BTreeSet, VecDeque};

use etas_types::{CheckedSpecRef, SpecKind, Type, TypeId, TypeOutput};

#[derive(Clone, Default, PartialEq, Eq)]
struct Shape {
    callable: bool,
    unresolved: bool,
    parameters: BTreeSet<TypeId>,
}

impl Shape {
    fn union(&mut self, other: &Self) {
        self.callable |= other.callable;
        self.unresolved |= other.unresolved;
        self.parameters.extend(&other.parameters);
    }
}

pub(crate) struct LatentTypeShapes {
    shapes: Vec<Shape>,
    derived: std::sync::Mutex<Vec<Shape>>,
    opaque_parameters: BTreeSet<TypeId>,
}

impl LatentTypeShapes {
    pub(crate) fn build(types: &TypeOutput) -> Self {
        let specs = etas_types::SpecFacts::from(types.facts.as_ref());
        let mut result = Self {
            derived: Default::default(),
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
                        }) && !etas_types::numeric_family::supports(
                            &types.store,
                            &specs,
                            ty,
                        ) && !etas_types::data_equality::parameter_has_equality(
                            &types.store,
                            &specs,
                            ty,
                        ) =>
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
            let shape = result.transfer(types, ty, &[]);
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

    fn with_shape<R>(&self, types: &TypeOutput, ty: TypeId, read: impl FnOnce(&Shape) -> R) -> R {
        let index = ty.0 as usize;
        if let Some(shape) = self.shapes.get(index) {
            return read(shape);
        }
        // Structural substitutions only refer to declarations and previously interned children.
        let mut derived = self.derived.lock().unwrap();
        let offset = index - self.shapes.len();
        while derived.len() <= offset {
            let next = TypeId((self.shapes.len() + derived.len()) as u32);
            let shape = self.transfer(types, next, &derived);
            derived.push(shape);
        }
        read(&derived[offset])
    }

    pub(crate) fn data_payload_parameters(
        &self,
        types: &TypeOutput,
        ty: TypeId,
    ) -> Option<Vec<TypeId>> {
        self.with_shape(types, ty, |shape| {
            (!shape.callable && !shape.unresolved)
                .then(|| shape.parameters.iter().copied().collect())
        })
    }

    pub(crate) fn contains(&self, types: &TypeOutput, ty: TypeId) -> bool {
        self.with_shape(types, ty, |shape| {
            shape.callable
                || shape
                    .parameters
                    .iter()
                    .any(|ty| self.opaque_parameters.contains(ty))
        })
    }

    pub(crate) fn has_parameters(&self, types: &TypeOutput, ty: TypeId) -> bool {
        self.with_shape(types, ty, |shape| !shape.parameters.is_empty())
    }

    fn transfer(&self, types: &TypeOutput, ty: TypeId, derived: &[Shape]) -> Shape {
        let value = types.store.get(ty).expect("indexed checked type shape");
        let descriptor = matches!(value, Type::Applied { constructor, args }
                if args.len() == 1 && [types.facts.known_std_types.code.callable_definition,
                    types.facts.known_std_types.code.agent_definition].contains(&Some(TypeId(constructor.0))));
        let child_shape = |id: TypeId| {
            let index = id.0 as usize;
            self.shapes
                .get(index)
                .unwrap_or_else(|| &derived[index - self.shapes.len()])
        };
        let mut shape = Shape::default();
        if etas_types::source_execution::is_scoped_handle(
            &types.store,
            &types.facts.known_std_types,
            ty,
        ) {
            return shape;
        }
        if descriptor || matches!(value, Type::Function(_)) {
            shape.callable = true;
        } else if let Type::Existential { bounds } = value {
            // A marker package retains type evidence but exposes no callable
            // members. A private marker can therefore appear in a data record
            // without making every constructor and field update a callback.
            shape.callable = bounds.iter().any(|bound| !marker_spec(types, &bound.spec));
        } else if matches!(value, Type::Var(_)) {
            shape.unresolved = true;
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
            let template = child_shape(owner);
            shape.callable = template.callable;
            shape.unresolved = template.unresolved;
            for parameter in &template.parameters {
                if let Some(position) = parameters
                    .iter()
                    .position(|binding| binding.subject == *parameter)
                {
                    shape.union(child_shape(args[position]));
                } else {
                    shape.parameters.insert(*parameter);
                }
            }
        } else if matches!(value, Type::Enum(_)) && !types.facts.enum_layouts.contains_key(&ty) {
            shape.callable = true;
        } else {
            for child in children(types, ty, value) {
                shape.union(child_shape(child));
            }
        }
        shape
    }
}

fn marker_spec(types: &TypeOutput, spec: &CheckedSpecRef) -> bool {
    let facts = &types.facts;
    let external = |identity| {
        facts
            .external_specs
            .signatures
            .get(identity)
            .is_some_and(|signature| {
                signature.kind == SpecKind::TypeSpec
                    && signature.methods.is_empty()
                    && signature.super_specs.is_empty()
            })
    };
    let source = |symbol| {
        facts.spec_signatures.get(symbol).is_some_and(|signature| {
            signature.kind == SpecKind::TypeSpec
                && signature.methods.is_empty()
                && signature.super_specs.is_empty()
        })
    };
    match spec {
        CheckedSpecRef::Source(symbol) => facts
            .external_specs
            .aliases
            .get(symbol)
            .map_or_else(|| source(symbol), external),
        CheckedSpecRef::External(identity) => external(identity),
        CheckedSpecRef::Std(path) => facts
            .std_spec_aliases
            .iter()
            .any(|(symbol, alias)| alias == path && source(symbol)),
        CheckedSpecRef::Canonical(_) => false,
    }
}

fn children(types: &TypeOutput, ty: TypeId, value: &Type) -> Vec<TypeId> {
    if etas_types::source_execution::is_scoped_handle(
        &types.store,
        &types.facts.known_std_types,
        ty,
    ) {
        return Vec::new();
    }
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
        | Type::ResourceHandle(_)
        | Type::AgentContext(_) => Vec::new(),
    }
}
