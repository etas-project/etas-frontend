//! Evidence for the existing equality of finite data, independent of a user's
//! implementation address. Hash inherits this requirement through Eq.
use crate::{CheckedSpecRef, PrimitiveType, SpecFacts, Type, TypeFacts, TypeId, TypeStore};
use std::collections::{BTreeSet, VecDeque};

/// The checked standard Hash member, including its inherited appearances.
/// A user spec with the same spelling does not establish this contract.
pub fn is_hash_member(facts: &SpecFacts, member: &crate::SpecMethodIdentity) -> bool {
    let is_hash = |path: &[String]| path == ["std", "collections", "Hash"];
    facts.std_spec_aliases.iter().any(|(symbol, path)| {
        is_hash(path)
            && facts
                .signatures
                .get(symbol)
                .is_some_and(|spec| spec.methods.iter().any(|method| &method.identity == member))
    }) || facts.external_std_specs.keys().any(|identity| {
        is_hash(&identity.path)
            && facts
                .external_signatures
                .get(identity)
                .is_some_and(|spec| spec.methods.iter().any(|method| &method.identity == member))
    })
}

pub fn requires_data(facts: &SpecFacts, root: &CheckedSpecRef) -> bool {
    let is_eq = |path: &[String]| {
        path == ["std", "collections", "Eq"] || path == ["std", "collections", "Hash"]
    };
    let mut pending = vec![root.clone()];
    let mut seen = BTreeSet::new();
    while let Some(spec) = pending.pop() {
        if !seen.insert(spec.clone()) {
            continue;
        }
        match spec {
            CheckedSpecRef::Std(path) => {
                if is_eq(&path) {
                    return true;
                }
                for (symbol, alias) in &facts.std_spec_aliases {
                    if *alias == path {
                        pending.push(CheckedSpecRef::Source(*symbol));
                    }
                }
            }
            CheckedSpecRef::Source(symbol) => {
                if facts
                    .std_spec_aliases
                    .get(&symbol)
                    .is_some_and(|path| is_eq(path))
                {
                    return true;
                }
                if let Some(identity) = facts.external_aliases.get(&symbol) {
                    pending.push(CheckedSpecRef::External(identity.clone()));
                }
                if let Some(signature) = facts.signatures.get(&symbol) {
                    pending.extend(
                        signature
                            .super_specs
                            .iter()
                            .map(|bound| bound.super_spec.clone()),
                    );
                }
            }
            CheckedSpecRef::External(identity) => {
                if (facts.external_std_specs.contains_key(&identity) && is_eq(&identity.path))
                    || facts.external_aliases.iter().any(|(symbol, candidate)| {
                        candidate == &identity
                            && facts
                                .std_spec_aliases
                                .get(symbol)
                                .is_some_and(|path| is_eq(path))
                    })
                {
                    return true;
                }
                if let Some(signature) = facts.external_signatures.get(&identity) {
                    pending.extend(signature.super_specs.iter().map(|bound| bound.spec.clone()));
                }
            }
            CheckedSpecRef::Canonical(identity) => {
                if facts
                    .canonical_std_specs
                    .get(&identity)
                    .is_some_and(|path| is_eq(path))
                {
                    return true;
                }
                if let Some(signature) = facts.canonical.signatures.get(&identity) {
                    pending.extend(signature.super_specs.iter().map(|bound| bound.spec.clone()));
                }
            }
        }
    }
    false
}

pub fn parameter_has_equality(store: &TypeStore, facts: &SpecFacts, ty: TypeId) -> bool {
    if crate::numeric_family::supports_family(
        store,
        facts,
        ty,
        crate::numeric_family::Family::Integer,
    ) {
        return true;
    }
    match store.get(ty) {
        Some(Type::Named(name)) => name.source_parameter.is_some_and(|symbol| {
            facts
                .type_param_bounds
                .get(&symbol)
                .into_iter()
                .flatten()
                .any(|bound| requires_data(facts, &CheckedSpecRef::Source(bound.spec_symbol)))
        }),
        Some(Type::Existential { bounds }) => {
            bounds.iter().any(|bound| requires_data(facts, &bound.spec))
        }
        _ => false,
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
struct Shape {
    invalid: bool,
    demands: BTreeSet<TypeId>,
}
impl Shape {
    fn include(&mut self, other: &Self) {
        self.invalid |= other.invalid;
        self.demands.extend(&other.demands);
    }
}

/// Summarize generic demand once. Recursive applications substitute demand
/// positions instead of repeatedly expanding their possibly growing arguments.
pub struct DataEquality {
    shapes: Vec<Shape>,
}
impl DataEquality {
    pub fn build(store: &TypeStore, facts: &TypeFacts) -> Self {
        let mut result = Self {
            shapes: vec![Shape::default(); store.len()],
        };
        let mut parents = vec![Vec::new(); store.len()];
        for (ty, value) in store.iter() {
            for child in children(facts, ty, value) {
                parents[child.0 as usize].push(ty);
            }
        }
        let mut pending = store.iter().map(|(ty, _)| ty).collect::<VecDeque<_>>();
        let mut queued = vec![true; store.len()];
        while let Some(ty) = pending.pop_front() {
            queued[ty.0 as usize] = false;
            let value = store.get(ty).unwrap();
            let mut shape = Shape::default();
            match value {
                Type::Primitive(PrimitiveType::F32 | PrimitiveType::F64) => shape.invalid = true,
                Type::Primitive(_) | Type::IntegerLiteral { .. } => {}
                Type::Named(_) | Type::Existential { .. } => {
                    shape.demands.insert(ty);
                }
                Type::Nominal(owner) if owner.representation.is_none() => {
                    shape.demands.insert(ty);
                }
                Type::Enum(_) if !facts.enum_layouts.contains_key(&ty) => {
                    shape.demands.insert(ty);
                }
                Type::Applied { constructor, args } => {
                    let owner = TypeId(constructor.0);
                    let template = &result.shapes[owner.0 as usize];
                    shape.invalid = template.invalid;
                    let parameters = facts
                        .type_generics
                        .get(&owner)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]);
                    for demand in &template.demands {
                        if *demand == owner {
                            // Imported opaque data needs its checked conformance
                            // for this exact application, including generic bounds.
                            shape.demands.insert(ty);
                        } else if let Some(position) = parameters
                            .iter()
                            .position(|parameter| parameter.subject == *demand)
                        {
                            shape.include(&result.shapes[args[position].0 as usize]);
                        } else {
                            shape.demands.insert(*demand);
                        }
                    }
                }
                Type::Array(_)
                | Type::List(_)
                | Type::Set(_)
                | Type::Slice(_)
                | Type::Option(_)
                | Type::Map { .. }
                | Type::Range { .. }
                | Type::Result { .. }
                | Type::Record(_)
                | Type::Tuple(_)
                | Type::Nominal(_)
                | Type::Enum(_)
                | Type::Refined { .. }
                | Type::Trust {
                    wrapper: crate::TrustWrapper::Public | crate::TrustWrapper::Untrusted,
                    ..
                } => {
                    for child in children(facts, ty, value) {
                        shape.include(&result.shapes[child.0 as usize]);
                    }
                }
                _ => shape.invalid = true,
            }
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

    pub fn supports(&self, store: &TypeStore, facts: &SpecFacts, ty: TypeId) -> bool {
        let shape = &self.shapes[ty.0 as usize];
        !shape.invalid
            && shape.demands.iter().all(|demand| {
                parameter_has_equality(store, facts, *demand)
                    || imported_equality(store, facts, *demand)
            })
    }
}

fn imported_equality(store: &TypeStore, facts: &SpecFacts, ty: TypeId) -> bool {
    use crate::solver::spec_solver::{
        implementation_bounds_satisfied, match_implementation_subject,
    };
    let applicable =
        |parameters: &[crate::CallableGenericParam], subject: TypeId, spec: &CheckedSpecRef| {
            requires_data(facts, spec)
                && match_implementation_subject(store, parameters, subject, ty).is_some_and(
                    |bindings| implementation_bounds_satisfied(store, facts, parameters, &bindings),
                )
        };
    facts.external_impls.iter().any(|implementation| {
        applicable(
            &implementation.generic_params,
            implementation.self_type,
            &CheckedSpecRef::External(implementation.spec.clone()),
        )
    }) || facts.canonical.impls.iter().any(|implementation| {
        applicable(
            &implementation.generic_params,
            implementation.self_type,
            &CheckedSpecRef::Canonical(implementation.spec.clone()),
        )
    }) || facts.checked_impls.iter().any(|implementation| {
        !matches!(implementation.spec, CheckedSpecRef::Source(_))
            && applicable(
                &implementation.generic_params,
                implementation.self_type,
                &implementation.spec,
            )
    })
}

fn children(facts: &TypeFacts, ty: TypeId, value: &Type) -> Vec<TypeId> {
    match value {
        Type::Array(inner)
        | Type::List(inner)
        | Type::Set(inner)
        | Type::Slice(inner)
        | Type::Option(inner)
        | Type::Range { index: inner }
        | Type::Refined { base: inner, .. }
        | Type::Trust { inner, .. } => vec![*inner],
        Type::Map { key, value }
        | Type::Result {
            ok: key,
            err: value,
        } => vec![*key, *value],
        Type::Tuple(elements) => elements.clone(),
        Type::Record(record) => record.fields.iter().map(|field| field.ty).collect(),
        Type::Nominal(owner) => owner.representation.into_iter().collect(),
        Type::Applied { constructor, args } => std::iter::once(TypeId(constructor.0))
            .chain(args.iter().copied())
            .collect(),
        Type::Enum(_) => facts
            .enum_layouts
            .get(&ty)
            .into_iter()
            .flat_map(|layout| &layout.variants)
            .flat_map(|variant| variant.fields.iter().copied())
            .collect(),
        _ => Vec::new(),
    }
}
