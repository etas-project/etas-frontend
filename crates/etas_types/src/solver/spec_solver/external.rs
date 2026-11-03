use super::*;
use crate::facts::specs::ExternalSpecIdentity;

type ActiveObligations = HashSet<(TypeId, CheckedSpecRef, Vec<TypeId>)>;

pub(super) fn canonical_type_satisfies(
    store: &TypeStore,
    facts: &SpecFacts,
    ty: TypeId,
    spec: &etas_code::DefinitionKey,
    args: &[TypeId],
) -> bool {
    canonical_type_satisfies_inner(store, facts, ty, spec, args, &mut HashSet::new())
}

fn canonical_type_satisfies_inner(
    store: &TypeStore,
    facts: &SpecFacts,
    ty: TypeId,
    spec: &etas_code::DefinitionKey,
    args: &[TypeId],
    visited: &mut ActiveObligations,
) -> bool {
    if spec.kind() != etas_code::DefinitionKind::Spec {
        return false;
    }
    let key = (ty, CheckedSpecRef::Canonical(spec.clone()), args.to_vec());
    if !visited.insert(key.clone()) {
        return false;
    }
    let result = canonical_arguments_satisfy(store, facts, spec, args, visited)
        && evidence_satisfies_inner(
            store,
            facts,
            ty,
            &CheckedSpecRef::Canonical(spec.clone()),
            args,
            visited,
        );
    visited.remove(&key);
    result
}

fn canonical_arguments_satisfy(
    store: &TypeStore,
    facts: &SpecFacts,
    spec: &etas_code::DefinitionKey,
    args: &[TypeId],
    visited: &mut ActiveObligations,
) -> bool {
    (|| {
        let Some(signature) = facts.canonical.signatures.get(spec) else {
            return false;
        };
        if signature.kind != crate::SpecKind::TypeSpec
            || signature.param_names.len() != args.len()
            || signature.param_kinds.len() != args.len()
            || signature.param_bounds.len() != args.len()
            || signature
                .param_kinds
                .iter()
                .any(|kind| *kind != crate::CallableGenericParamKind::Type)
        {
            return false;
        }
        let mut bindings = HashMap::new();
        for (index, argument) in args.iter().copied().enumerate() {
            let Ok(index) = u32::try_from(index) else {
                return false;
            };
            let Ok(identity) = crate::ty::CanonicalTypeParameterIdentity::new(spec.clone(), index)
            else {
                return false;
            };
            bindings.insert(identity, argument);
        }
        let names = HashMap::new();
        let mut substitution = crate::TypeSubstitutionEngine::existing(store, &names)
            .with_canonical_parameters(&bindings);
        for (argument, bounds) in args.iter().copied().zip(&signature.param_bounds) {
            for bound in bounds {
                let Ok(bound_args) = bound
                    .args
                    .iter()
                    .map(|ty| substitution.substitute(*ty))
                    .collect::<Result<Vec<_>, _>>()
                else {
                    return false;
                };
                let satisfied = match &bound.spec {
                    CheckedSpecRef::Canonical(key) => canonical_type_satisfies_inner(
                        store,
                        facts,
                        argument,
                        key,
                        &bound_args,
                        visited,
                    ),
                    CheckedSpecRef::Source(symbol) => source_satisfies_inner(
                        store,
                        facts,
                        argument,
                        *symbol,
                        &bound_args,
                        visited,
                    ),
                    CheckedSpecRef::Std(path) => {
                        std_satisfies_inner(store, facts, argument, path, &bound_args, visited)
                    }
                    CheckedSpecRef::External(identity) => {
                        type_satisfies_inner(store, facts, argument, identity, &bound_args, visited)
                    }
                };
                if !satisfied {
                    return false;
                }
            }
        }
        true
    })()
}

pub(super) fn type_satisfies(
    store: &TypeStore,
    facts: &SpecFacts,
    ty: TypeId,
    spec: &ExternalSpecIdentity,
    args: &[TypeId],
) -> bool {
    type_satisfies_inner(store, facts, ty, spec, args, &mut HashSet::new())
}

fn type_satisfies_inner(
    store: &TypeStore,
    facts: &SpecFacts,
    ty: TypeId,
    spec: &ExternalSpecIdentity,
    args: &[TypeId],
    visited: &mut ActiveObligations,
) -> bool {
    let key = (ty, CheckedSpecRef::External(spec.clone()), args.to_vec());
    if !visited.insert(key.clone()) {
        return false;
    }
    let result = external_arguments_satisfy(store, facts, spec, args, visited)
        && evidence_satisfies_inner(
            store,
            facts,
            ty,
            &CheckedSpecRef::External(spec.clone()),
            args,
            visited,
        );
    visited.remove(&key);
    result
}

fn external_arguments_satisfy(
    store: &TypeStore,
    facts: &SpecFacts,
    spec: &ExternalSpecIdentity,
    args: &[TypeId],
    visited: &mut ActiveObligations,
) -> bool {
    let Some(signature) = facts.external_signatures.get(spec) else {
        return false;
    };
    if signature.kind != crate::SpecKind::TypeSpec
        || signature.param_names.len() != args.len()
        || signature.generic_params.len() != args.len()
    {
        return false;
    }
    let bindings = signature
        .generic_params
        .iter()
        .zip(args)
        .map(|(param, arg)| (param.name.clone(), *arg))
        .collect::<HashMap<_, _>>();
    let mut substitution = crate::TypeSubstitutionEngine::existing(store, &bindings);
    for ((name, param), argument) in signature
        .param_names
        .iter()
        .zip(&signature.generic_params)
        .zip(args.iter().copied())
    {
        if param.name != *name || param.kind != crate::CallableGenericParamKind::Type {
            return false;
        }
        for bound in &param.bounds {
            let Ok(bound_args) = bound
                .args
                .iter()
                .map(|ty| substitution.substitute(*ty))
                .collect::<Result<Vec<_>, _>>()
            else {
                return false;
            };
            let satisfied = match &bound.spec {
                CheckedSpecRef::Canonical(key) => canonical_type_satisfies_inner(
                    store,
                    facts,
                    argument,
                    key,
                    &bound_args,
                    visited,
                ),
                CheckedSpecRef::External(identity) => {
                    type_satisfies_inner(store, facts, argument, identity, &bound_args, visited)
                }
                CheckedSpecRef::Source(symbol) => {
                    source_satisfies_inner(store, facts, argument, *symbol, &bound_args, visited)
                }
                CheckedSpecRef::Std(path) => {
                    std_satisfies_inner(store, facts, argument, path, &bound_args, visited)
                }
            };
            if !satisfied {
                return false;
            }
        }
    }
    true
}

fn source_satisfies_inner(
    store: &TypeStore,
    facts: &SpecFacts,
    ty: TypeId,
    spec: etas_hir::SymbolId,
    args: &[TypeId],
    visited: &mut ActiveObligations,
) -> bool {
    match normalize(facts, CheckedSpecRef::Source(spec)) {
        CheckedSpecRef::External(identity) => {
            return type_satisfies_inner(store, facts, ty, &identity, args, visited);
        }
        CheckedSpecRef::Std(path) => {
            return std_satisfies_inner(store, facts, ty, &path, args, visited);
        }
        _ => {}
    }
    let target = CheckedSpecRef::Source(spec);
    let key = (ty, target.clone(), args.to_vec());
    if !visited.insert(key.clone()) {
        return false;
    }
    let result = source_arguments_satisfy(store, facts, spec, args, visited)
        && evidence_satisfies_inner(store, facts, ty, &target, args, visited);
    visited.remove(&key);
    result
}

fn source_arguments_satisfy(
    store: &TypeStore,
    facts: &SpecFacts,
    spec: etas_hir::SymbolId,
    args: &[TypeId],
    visited: &mut ActiveObligations,
) -> bool {
    let Some(signature) = facts.signatures.get(&spec) else {
        return false;
    };
    if signature.kind != crate::SpecKind::TypeSpec
        || signature.params.len() != args.len()
        || signature.param_names.len() != args.len()
    {
        return false;
    }
    let names = signature
        .param_names
        .iter()
        .cloned()
        .zip(args.iter().copied())
        .collect::<HashMap<_, _>>();
    let parameters = signature
        .params
        .iter()
        .copied()
        .zip(args.iter().copied())
        .collect::<HashMap<_, _>>();
    let mut substitution =
        crate::TypeSubstitutionEngine::existing(store, &names).with_source_parameters(&parameters);
    for (parameter, argument) in signature.params.iter().zip(args.iter().copied()) {
        for bound in facts.type_param_bounds.get(parameter).into_iter().flatten() {
            let Ok(bound_args) = bound
                .args
                .iter()
                .map(|ty| substitution.substitute(*ty))
                .collect::<Result<Vec<_>, _>>()
            else {
                return false;
            };
            if !source_satisfies_inner(
                store,
                facts,
                argument,
                bound.spec_symbol,
                &bound_args,
                visited,
            ) {
                return false;
            }
        }
    }
    true
}

fn std_satisfies_inner(
    store: &TypeStore,
    facts: &SpecFacts,
    ty: TypeId,
    spec: &[String],
    args: &[TypeId],
    visited: &mut ActiveObligations,
) -> bool {
    let target = CheckedSpecRef::Std(spec.to_vec());
    let key = (ty, target.clone(), args.to_vec());
    if !visited.insert(key.clone()) {
        return false;
    }
    let result = std_type_param_bound_satisfies_spec(store, facts, ty, spec, args)
        || evidence_satisfies_inner(store, facts, ty, &target, args, visited)
        || facts.std_impls.iter().any(|implementation| {
            implementation.spec == spec
                && type_same(store, implementation.self_type, ty)
                && args_match(store, &implementation.args, args)
        })
        || facts.impls.iter().any(|implementation| {
            facts
                .std_spec_aliases
                .get(&implementation.spec_symbol)
                .is_some_and(|path| path == spec)
                && type_same(store, implementation.self_type, ty)
                && args_match(store, &implementation.args, args)
        });
    visited.remove(&key);
    result
}

pub(super) fn evidence_satisfies(
    store: &TypeStore,
    facts: &SpecFacts,
    ty: TypeId,
    target: &CheckedSpecRef,
    args: &[TypeId],
) -> bool {
    evidence_satisfies_inner(store, facts, ty, target, args, &mut HashSet::new())
}

fn evidence_satisfies_inner(
    store: &TypeStore,
    facts: &SpecFacts,
    ty: TypeId,
    target: &CheckedSpecRef,
    args: &[TypeId],
    active: &mut ActiveObligations,
) -> bool {
    let external_impl = facts.external_impls.iter().any(|implementation| {
        type_same(store, implementation.self_type, ty)
            && entails_inner(
                store,
                facts,
                CheckedSpecRef::External(implementation.spec.clone()),
                &implementation.args,
                target,
                args,
                active,
            )
    });
    if external_impl {
        return true;
    }
    if facts.canonical.impls.iter().any(|implementation| {
        type_same(store, implementation.self_type, ty)
            && entails_inner(
                store,
                facts,
                CheckedSpecRef::Canonical(implementation.spec.clone()),
                &implementation.args,
                target,
                args,
                active,
            )
    }) {
        return true;
    }
    if facts.impls.iter().any(|implementation| {
        type_same(store, implementation.self_type, ty)
            && entails_inner(
                store,
                facts,
                CheckedSpecRef::Source(implementation.spec_symbol),
                &implementation.args,
                target,
                args,
                active,
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
            entails_inner(
                store,
                facts,
                CheckedSpecRef::Source(bound.spec_symbol),
                &bound.args,
                target,
                args,
                active,
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
    entails_inner(
        store,
        facts,
        current,
        args,
        target,
        expected,
        &mut HashSet::new(),
    )
}

fn entails_inner(
    store: &TypeStore,
    facts: &SpecFacts,
    current: CheckedSpecRef,
    args: &[TypeId],
    target: &CheckedSpecRef,
    expected: &[TypeId],
    active: &mut ActiveObligations,
) -> bool {
    let target = normalize(facts, target.clone());
    let mut pending = vec![(current, args.to_vec())];
    let mut visited = HashSet::new();
    while let Some((current, args)) = pending.pop() {
        let current = normalize(facts, current);
        if !visited.insert((current.clone(), args.clone())) {
            continue;
        }
        let arguments_valid = match &current {
            CheckedSpecRef::Canonical(identity) => {
                canonical_arguments_satisfy(store, facts, identity, &args, active)
            }
            CheckedSpecRef::External(identity) => {
                external_arguments_satisfy(store, facts, identity, &args, active)
            }
            CheckedSpecRef::Source(symbol) => {
                source_arguments_satisfy(store, facts, *symbol, &args, active)
            }
            _ => true,
        };
        if !arguments_valid {
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
            CheckedSpecRef::Canonical(identity) => {
                let Some(signature) = facts.canonical.signatures.get(identity) else {
                    return false;
                };
                (&signature.param_names, signature.super_specs.clone())
            }
            CheckedSpecRef::Std(_) => continue,
        };
        if names.len() != args.len() {
            return false;
        }
        let bindings = names
            .iter()
            .cloned()
            .zip(args.iter().copied())
            .collect::<HashMap<_, _>>();
        let mut canonical = HashMap::new();
        if let CheckedSpecRef::Canonical(owner) = &current {
            for (index, argument) in args.iter().copied().enumerate() {
                let Ok(index) = u32::try_from(index) else {
                    return false;
                };
                let Ok(identity) =
                    crate::ty::CanonicalTypeParameterIdentity::new(owner.clone(), index)
                else {
                    return false;
                };
                canonical.insert(identity, argument);
            }
        }
        for bound in supers {
            let args = bound
                .args
                .iter()
                .map(|ty| {
                    crate::TypeSubstitutionEngine::existing(store, &bindings)
                        .with_canonical_parameters(&canonical)
                        .substitute(*ty)
                })
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
