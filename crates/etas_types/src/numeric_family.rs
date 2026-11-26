//! Checked evidence for the closed primitive numeric family. An ordinary
//! same-named spec or an explicit user impl cannot grant numeric operations.
use crate::{CheckedSpecRef, PrimitiveType, SpecFacts, Type, TypeId, TypeStore};

pub fn primitive(primitive: PrimitiveType) -> bool {
    matches!(
        primitive,
        PrimitiveType::I8
            | PrimitiveType::I16
            | PrimitiveType::I32
            | PrimitiveType::I64
            | PrimitiveType::I128
            | PrimitiveType::ISize
            | PrimitiveType::U8
            | PrimitiveType::U16
            | PrimitiveType::U32
            | PrimitiveType::U64
            | PrimitiveType::U128
            | PrimitiveType::USize
            | PrimitiveType::F32
            | PrimitiveType::F64
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Number,
    Integer,
    Float,
}

pub fn spec_family(facts: &SpecFacts, spec: &CheckedSpecRef) -> Option<Family> {
    let path = match spec {
        CheckedSpecRef::Std(path) => Some(path),
        CheckedSpecRef::Source(symbol) => facts.std_spec_aliases.get(symbol),
        CheckedSpecRef::External(identity) => {
            if facts.external_std_specs.contains_key(identity) {
                Some(&identity.path)
            } else {
                facts
                    .external_aliases
                    .iter()
                    .find_map(|(symbol, candidate)| {
                        (candidate == identity)
                            .then(|| facts.std_spec_aliases.get(symbol))
                            .flatten()
                    })
            }
        }
        CheckedSpecRef::Canonical(identity) => facts.canonical_std_specs.get(identity),
    }?;
    if path.len() != 3 || path[0] != "std" || path[1] != "math" {
        return None;
    }
    match path[2].as_str() {
        "Number" => Some(Family::Number),
        "Integer" => Some(Family::Integer),
        "Float" => Some(Family::Float),
        _ => None,
    }
}

pub fn is_number_spec(facts: &SpecFacts, spec: &CheckedSpecRef) -> bool {
    spec_family(facts, spec) == Some(Family::Number)
}

pub fn supports(store: &TypeStore, facts: &SpecFacts, ty: TypeId) -> bool {
    supports_family(store, facts, ty, Family::Number)
}

pub fn supports_family(store: &TypeStore, facts: &SpecFacts, ty: TypeId, target: Family) -> bool {
    supports_family_with(store, ty, target, || facts)
}

pub(crate) fn supports_family_with<F: std::borrow::Borrow<SpecFacts>>(
    store: &TypeStore,
    ty: TypeId,
    target: Family,
    facts: impl FnOnce() -> F,
) -> bool {
    if let Some(Type::Primitive(kind)) = store.get(ty) {
        return primitive(*kind)
            && match target {
                Family::Number => true,
                Family::Integer => !matches!(kind, PrimitiveType::F32 | PrimitiveType::F64),
                Family::Float => matches!(kind, PrimitiveType::F32 | PrimitiveType::F64),
            };
    }
    let Some(Type::Named(name)) = store.get(ty) else {
        return false;
    };
    let Some(parameter) = name.source_parameter else {
        return false;
    };
    let facts = facts();
    let facts = facts.borrow();
    let mut pending: Vec<_> = facts
        .type_param_bounds
        .get(&parameter)
        .into_iter()
        .flatten()
        .map(|bound| CheckedSpecRef::Source(bound.spec_symbol))
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    while let Some(spec) = pending.pop() {
        if !seen.insert(spec.clone()) {
            continue;
        }
        if spec_family(facts, &spec)
            .is_some_and(|family| family == target || target == Family::Number)
        {
            return true;
        }
        match spec {
            CheckedSpecRef::Source(symbol) => {
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
            _ => {}
        }
    }
    false
}
