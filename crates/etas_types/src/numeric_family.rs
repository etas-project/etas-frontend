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

pub fn is_number_spec(facts: &SpecFacts, spec: &CheckedSpecRef) -> bool {
    let path_is_number = |path: &[String]| path == ["std", "math", "Number"];
    match spec {
        CheckedSpecRef::Std(path) => path_is_number(path),
        CheckedSpecRef::Source(symbol) => facts
            .std_spec_aliases
            .get(symbol)
            .is_some_and(|path| path_is_number(path)),
        CheckedSpecRef::External(identity) => {
            facts.external_aliases.iter().any(|(symbol, candidate)| {
                candidate == identity
                    && facts
                        .std_spec_aliases
                        .get(symbol)
                        .is_some_and(|path| path_is_number(path))
            })
        }
        CheckedSpecRef::Canonical(_) => false,
    }
}

pub fn supports(store: &TypeStore, facts: &SpecFacts, ty: TypeId) -> bool {
    if let Some(Type::Primitive(kind)) = store.get(ty) {
        return primitive(*kind);
    }
    let Some(Type::Named(name)) = store.get(ty) else {
        return false;
    };
    let Some(parameter) = name.source_parameter else {
        return false;
    };
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
        if is_number_spec(facts, &spec) {
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
