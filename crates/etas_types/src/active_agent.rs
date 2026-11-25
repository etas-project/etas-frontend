use crate::{CheckedSpecRef, SpecFacts, Type, TypeId, TypeStore};

pub fn is_active_spec(facts: &SpecFacts, spec: &CheckedSpecRef) -> bool {
    is_agent_spec(facts, spec, "Active")
}

fn is_agent_spec(facts: &SpecFacts, spec: &CheckedSpecRef, name: &str) -> bool {
    let path = match spec {
        CheckedSpecRef::Std(path) => Some(path),
        CheckedSpecRef::Source(symbol) => facts.std_spec_aliases.get(symbol),
        CheckedSpecRef::External(identity) => facts
            .external_std_specs
            .contains_key(identity)
            .then_some(&identity.path),
        CheckedSpecRef::Canonical(identity) => facts.canonical_std_specs.get(identity),
    };
    path.is_some_and(|path| path == &["std", "agent", name])
}

pub fn supports(store: &TypeStore, facts: &SpecFacts, ty: TypeId) -> bool {
    let constructor = match store.get(ty) {
        Some(Type::Applied { constructor, .. }) => TypeId(constructor.0),
        _ => ty,
    };
    if facts.active_agents.contains(&constructor) {
        return true;
    }
    let mut pending = match store.get(ty) {
        Some(Type::Named(named)) => named
            .source_parameter
            .into_iter()
            .flat_map(|parameter| {
                facts
                    .type_param_bounds
                    .get(&parameter)
                    .into_iter()
                    .flatten()
            })
            .map(|bound| CheckedSpecRef::Source(bound.spec_symbol))
            .collect::<Vec<_>>(),
        Some(Type::Existential { bounds }) => {
            bounds.iter().map(|bound| bound.spec.clone()).collect()
        }
        _ => Vec::new(),
    };
    let mut seen = std::collections::BTreeSet::new();
    while let Some(spec) = pending.pop() {
        if !seen.insert(spec.clone()) {
            continue;
        }
        if is_active_spec(facts, &spec) {
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
            CheckedSpecRef::Canonical(identity) => {
                if let Some(signature) = facts.canonical.signatures.get(&identity) {
                    pending.extend(signature.super_specs.iter().map(|bound| bound.spec.clone()));
                }
            }
            CheckedSpecRef::Std(_) => {}
        }
    }
    false
}

/// Resolve the standard role through the ordinary checked implementation witness.
/// A similarly named application spec or inherent main is not an automatic entry.
pub fn autonomous_main<'a>(
    types: &'a crate::TypeFacts,
    store: &TypeStore,
    facts: &SpecFacts,
    ty: TypeId,
) -> Result<Option<crate::method::MethodWitness<'a>>, String> {
    if !types.is_active_agent(store, ty) {
        return Ok(None);
    }
    let member = types
        .methods
        .member("main")
        .find_map(|(_, method)| match &method.target {
            crate::method::MethodTarget::BoundSpec { spec, member, .. }
                if is_agent_spec(facts, spec, "Autonomous") =>
            {
                Some(member)
            }
            _ => None,
        })
        .or_else(|| {
            facts.signatures.iter().find_map(|(symbol, signature)| {
                is_agent_spec(facts, &CheckedSpecRef::Source(*symbol), "Autonomous")
                    .then(|| {
                        signature
                            .methods
                            .iter()
                            .find(|method| method.name == "main")
                    })
                    .flatten()
                    .map(|method| &method.identity)
            })
        })
        .or_else(|| {
            facts
                .external_signatures
                .iter()
                .find_map(|(identity, signature)| {
                    is_agent_spec(
                        facts,
                        &CheckedSpecRef::External(identity.clone()),
                        "Autonomous",
                    )
                    .then(|| {
                        signature
                            .methods
                            .iter()
                            .find(|method| method.name == "main")
                    })
                    .flatten()
                    .map(|method| &method.identity)
                })
        });
    let Some(member) = member else {
        return Ok(None);
    };
    types.methods.resolve_witness(store, facts, ty, &[], member)
}

/// Avoid materializing unrelated spec facts for classes with no main method.
pub fn has_main(types: &crate::TypeFacts, store: &TypeStore, ty: TypeId) -> bool {
    let constructor = |ty| match store.get(ty) {
        Some(Type::Applied { constructor, .. }) => TypeId(constructor.0),
        _ => ty,
    };
    let owner = constructor(ty);
    types
        .methods
        .member("main")
        .any(|(_, method)| constructor(method.receiver_type) == owner)
}
