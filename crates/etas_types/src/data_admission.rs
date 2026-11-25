//! Structural evidence for finite value schemas. Generic declarations summarize
//! their component demands once; applications substitute those demands rather
//! than repeatedly expanding recursive representations.
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use crate::{CheckedSpecRef, SpecFacts, TrustWrapper, Type, TypeFacts, TypeId, TypeStore};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Contract {
    Equality,
    StorageData,
    StorageKey,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct Declarations {
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    enums: HashMap<TypeId, crate::EnumLayoutFact>,
    #[serde(serialize_with = "etas_utils::serde_ordered::map")]
    parameters: HashMap<TypeId, Vec<crate::CallableGenericParam>>,
    trust: BTreeMap<TypeId, TrustWrapper>,
    excluded: BTreeSet<TypeId>,
}

impl Declarations {
    pub fn from_facts(facts: &TypeFacts) -> Self {
        let known = &facts.known_std_types;
        Self {
            enums: facts.enum_layouts.clone(),
            parameters: facts.type_generics.clone(),
            trust: facts.known_std_types.trust_wrappers.clone(),
            excluded: known
                .storage_resources
                .iter()
                .chain(&known.opaque_values)
                .copied()
                .chain(
                    [
                        known.execution.task,
                        known.execution.scheduler,
                        known.execution.continuation,
                        known.execution.task_handle,
                        known.execution.scheduler_state,
                        known.execution.continuation_state,
                    ]
                    .into_iter()
                    .flatten(),
                )
                .collect(),
        }
    }
}

fn standard_path<'a>(facts: &'a SpecFacts, spec: &'a CheckedSpecRef) -> Option<&'a [String]> {
    match spec {
        CheckedSpecRef::Std(path) => Some(path),
        CheckedSpecRef::Source(symbol) => facts.std_spec_aliases.get(symbol).map(Vec::as_slice),
        CheckedSpecRef::External(identity) => facts
            .external_std_specs
            .contains_key(identity)
            .then_some(identity.path.as_slice()),
        CheckedSpecRef::Canonical(identity) => {
            facts.canonical_std_specs.get(identity).map(Vec::as_slice)
        }
    }
}

pub fn contract(facts: &SpecFacts, spec: &CheckedSpecRef) -> Option<Contract> {
    let path = standard_path(facts, spec)?;
    match path
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["std", "collections", "Eq"] => Some(Contract::Equality),
        ["std", "memory", "StorageData"] => Some(Contract::StorageData),
        ["std", "memory", "StorageKey"] => Some(Contract::StorageKey),
        _ => None,
    }
}

pub fn requirements(facts: &SpecFacts, root: &CheckedSpecRef) -> BTreeSet<Contract> {
    let mut result = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut pending = vec![root.clone()];
    while let Some(spec) = pending.pop() {
        if !seen.insert(spec.clone()) {
            continue;
        }
        if let Some(kind) = contract(facts, &spec) {
            result.insert(kind);
        }
        match spec {
            CheckedSpecRef::Source(symbol) => {
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
                if let Some(signature) = facts.external_signatures.get(&identity) {
                    pending.extend(signature.super_specs.iter().map(|bound| bound.spec.clone()));
                }
            }
            CheckedSpecRef::Canonical(identity) => {
                if let Some(signature) = facts.canonical.signatures.get(&identity) {
                    pending.extend(signature.super_specs.iter().map(|bound| bound.spec.clone()));
                }
            }
            CheckedSpecRef::Std(path) => {
                pending.extend(
                    facts
                        .std_spec_aliases
                        .iter()
                        .filter(|(_, selected)| **selected == path)
                        .map(|(symbol, _)| CheckedSpecRef::Source(*symbol)),
                );
                pending.extend(
                    facts
                        .external_std_specs
                        .keys()
                        .filter(|identity| identity.path == path)
                        .cloned()
                        .map(CheckedSpecRef::External),
                );
            }
        }
    }
    if result.contains(&Contract::StorageKey) {
        result.extend([Contract::StorageData, Contract::Equality]);
    }
    result
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

pub struct Schema {
    shapes: Vec<Shape>,
}

impl Schema {
    pub fn build(store: &TypeStore, facts: &SpecFacts, kind: Contract) -> Self {
        assert_ne!(kind, Contract::StorageKey);
        let declarations = &facts.data_declarations;
        let mut shapes = vec![Shape::default(); store.len()];
        let mut parents = vec![Vec::new(); store.len()];
        for (ty, value) in store.iter() {
            for child in children(declarations, ty, value, kind) {
                parents[child.0 as usize].push(ty);
            }
        }
        let mut pending = store.iter().map(|(ty, _)| ty).collect::<VecDeque<_>>();
        let mut queued = vec![true; store.len()];
        while let Some(ty) = pending.pop_front() {
            queued[ty.0 as usize] = false;
            let value = store.get(ty).expect("schema type");
            let mut shape = Shape::default();
            let owner = match value {
                Type::Applied { constructor, .. } => TypeId(constructor.0),
                _ => ty,
            };
            if declarations.excluded.contains(&owner) {
                shape.invalid = true;
            } else {
                match value {
                    Type::Primitive(crate::PrimitiveType::F32 | crate::PrimitiveType::F64)
                        if kind == Contract::Equality =>
                    {
                        shape.invalid = true
                    }
                    Type::Primitive(_) | Type::IntegerLiteral { .. } => {}
                    Type::Named(_) | Type::Existential { .. } => {
                        shape.demands.insert(ty);
                    }
                    Type::Nominal(owner) if owner.representation.is_none() => {
                        shape.demands.insert(ty);
                    }
                    Type::Enum(_) if !declarations.enums.contains_key(&ty) => {
                        shape.demands.insert(ty);
                    }
                    Type::Applied { constructor, args } => {
                        let owner = TypeId(constructor.0);
                        if kind == Contract::StorageData
                            && let Some(wrapper) = declarations.trust.get(&owner)
                        {
                            if *wrapper == TrustWrapper::Secret {
                                shape.invalid = true;
                            } else {
                                shape.include(&shapes[args[0].0 as usize]);
                            }
                        } else {
                            let template = &shapes[owner.0 as usize];
                            shape.invalid = template.invalid;
                            let parameters = declarations
                                .parameters
                                .get(&owner)
                                .map_or(&[][..], Vec::as_slice);
                            for demand in &template.demands {
                                if *demand == owner {
                                    shape.demands.insert(ty);
                                } else if let Some(position) = parameters
                                    .iter()
                                    .position(|parameter| parameter.subject == *demand)
                                {
                                    shape.include(&shapes[args[position].0 as usize]);
                                } else {
                                    shape.demands.insert(*demand);
                                }
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
                    | Type::Refined { .. } => {
                        for child in children(declarations, ty, value, kind) {
                            shape.include(&shapes[child.0 as usize]);
                        }
                    }
                    Type::Trust { wrapper, inner } if *wrapper != TrustWrapper::Secret => {
                        if kind == Contract::StorageData
                            || matches!(wrapper, TrustWrapper::Public | TrustWrapper::Untrusted)
                        {
                            shape.include(&shapes[inner.0 as usize]);
                        } else {
                            shape.invalid = true;
                        }
                    }
                    _ => shape.invalid = true,
                }
            }
            if shapes[ty.0 as usize] != shape {
                shapes[ty.0 as usize] = shape;
                for parent in &parents[ty.0 as usize] {
                    if !queued[parent.0 as usize] {
                        queued[parent.0 as usize] = true;
                        pending.push_back(*parent);
                    }
                }
            }
        }
        Self { shapes }
    }

    pub fn supports(&self, ty: TypeId, mut demand: impl FnMut(TypeId) -> bool) -> bool {
        let shape = &self.shapes[ty.0 as usize];
        !shape.invalid && shape.demands.iter().copied().all(&mut demand)
    }
}

fn children(declarations: &Declarations, ty: TypeId, value: &Type, kind: Contract) -> Vec<TypeId> {
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
        Type::Tuple(items) => items.clone(),
        Type::Record(record) => record.fields.iter().map(|field| field.ty).collect(),
        Type::Nominal(owner) => owner.representation.into_iter().collect(),
        Type::Applied { constructor, args }
            if kind == Contract::StorageData
                && declarations.trust.contains_key(&TypeId(constructor.0)) =>
        {
            args.clone()
        }
        Type::Applied { constructor, args } => std::iter::once(TypeId(constructor.0))
            .chain(args.iter().copied())
            .collect(),
        Type::Enum(_) => declarations
            .enums
            .get(&ty)
            .into_iter()
            .flat_map(|layout| &layout.variants)
            .flat_map(|variant| variant.fields.iter().copied())
            .collect(),
        _ => Vec::new(),
    }
}
