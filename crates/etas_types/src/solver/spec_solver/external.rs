use super::*;
use crate::facts::specs::ExternalSpecIdentity;

pub(super) fn type_satisfies(
    store: &TypeStore,
    facts: &SpecFacts,
    ty: TypeId,
    spec: &ExternalSpecIdentity,
    args: &[TypeId],
) -> bool {
    let Some(signature) = facts.external_signatures.get(spec) else {
        return false;
    };
    signature.kind == crate::SpecKind::TypeSpec
        && signature.param_names.len() == args.len()
        && evidence_satisfies(
            store,
            facts,
            ty,
            &CheckedSpecRef::External(spec.clone()),
            args,
        )
}

pub(super) fn evidence_satisfies(
    store: &TypeStore,
    facts: &SpecFacts,
    ty: TypeId,
    target: &CheckedSpecRef,
    args: &[TypeId],
) -> bool {
    let external_impl = facts.external_impls.iter().any(|implementation| {
        type_same(store, implementation.self_type, ty)
            && entails(
                store,
                facts,
                CheckedSpecRef::External(implementation.spec.clone()),
                &implementation.args,
                target,
                args,
            )
    });
    if external_impl {
        return true;
    }
    if facts.impls.iter().any(|implementation| {
        type_same(store, implementation.self_type, ty)
            && entails(
                store,
                facts,
                CheckedSpecRef::Source(implementation.spec_symbol),
                &implementation.args,
                target,
                args,
            )
    }) {
        return true;
    }
    let Some(Type::Named(NamedTypeRef {
        source_parameter: Some(parameter),
        ..
    })) = store.get(ty)
    else {
        return false;
    };
    facts
        .type_param_bounds
        .get(parameter)
        .into_iter()
        .flatten()
        .any(|bound| {
            entails(
                store,
                facts,
                CheckedSpecRef::Source(bound.spec_symbol),
                &bound.args,
                target,
                args,
            )
        })
}

pub(super) fn entails(
    store: &TypeStore,
    facts: &SpecFacts,
    current: CheckedSpecRef,
    args: &[TypeId],
    target: &CheckedSpecRef,
    expected: &[TypeId],
) -> bool {
    let target = normalize(facts, target.clone());
    let mut pending = vec![(current, args.to_vec())];
    let mut visited = HashSet::new();
    while let Some((current, args)) = pending.pop() {
        let current = normalize(facts, current);
        if !visited.insert((current.clone(), args.clone())) {
            continue;
        }
        if current == target && args_match(store, &args, expected) {
            return true;
        }
        let (names, supers) = match &current {
            CheckedSpecRef::External(identity) => {
                let Some(signature) = facts.external_signatures.get(identity) else {
                    return false;
                };
                (&signature.param_names, signature.super_specs.clone())
            }
            CheckedSpecRef::Source(symbol) => {
                let Some(signature) = facts.signatures.get(symbol) else {
                    return false;
                };
                (
                    &signature.param_names,
                    signature
                        .super_specs
                        .iter()
                        .map(|bound| crate::CheckedSpecBound {
                            spec: bound.super_spec.clone(),
                            args: bound.args.clone(),
                        })
                        .collect(),
                )
            }
            CheckedSpecRef::Std(_) => continue,
        };
        if names.len() != args.len() {
            return false;
        }
        let bindings = names.iter().cloned().zip(args).collect::<HashMap<_, _>>();
        for bound in supers {
            let args = bound
                .args
                .iter()
                .map(|ty| crate::substitute_named_params_in_store(store, *ty, &bindings))
                .collect::<Result<Vec<_>, _>>();
            let Ok(args) = args else {
                return false;
            };
            pending.push((bound.spec, args));
        }
    }
    false
}

fn normalize(facts: &SpecFacts, spec: CheckedSpecRef) -> CheckedSpecRef {
    match spec {
        CheckedSpecRef::Source(symbol) => {
            if let Some(identity) = facts.external_aliases.get(&symbol) {
                CheckedSpecRef::External(identity.clone())
            } else if let Some(path) = facts.std_spec_aliases.get(&symbol) {
                CheckedSpecRef::Std(path.clone())
            } else {
                CheckedSpecRef::Source(symbol)
            }
        }
        spec => spec,
    }
}
