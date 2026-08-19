use std::collections::{HashMap, HashSet};

use crate::{
    CheckedSpecRef, NamedTypeRef, SpecFacts, SpecObligation, Type, TypeId, TypeStore, TypeUnifier,
    solver::report::{SolverFailure, SolverReport},
};

pub fn solve_spec_obligations(
    store: &TypeStore,
    facts: &SpecFacts,
    obligations: &[SpecObligation],
    named_substitutions: &HashMap<String, TypeId>,
) -> SolverReport {
    let mut report = SolverReport::default();
    for obligation in obligations {
        let ty = resolve_named(store, obligation.ty, named_substitutions);
        let args = obligation
            .args
            .iter()
            .copied()
            .map(|arg| resolve_named(store, arg, named_substitutions))
            .collect::<Vec<_>>();
        let mut visited = HashSet::new();
        let satisfied = match &obligation.spec {
            CheckedSpecRef::Source(spec_symbol) => {
                type_satisfies_spec(store, facts, ty, *spec_symbol, &args, &mut visited)
            }
            CheckedSpecRef::Std(path) => std_type_satisfies_spec(store, facts, ty, path, &args),
        };
        if !satisfied {
            report.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::TypeMismatch,
                span: obligation.span,
                message: format!(
                    "{} does not satisfy spec bound `{}`",
                    display_type_name(store, ty),
                    checked_spec_name(facts, &obligation.spec)
                ),
            });
        }
    }
    report
}

pub fn satisfies_spec(
    store: &TypeStore,
    facts: &SpecFacts,
    ty: TypeId,
    spec_symbol: etas_hir::SymbolId,
    args: &[TypeId],
) -> bool {
    type_satisfies_spec(store, facts, ty, spec_symbol, args, &mut HashSet::new())
}

fn type_satisfies_spec(
    store: &TypeStore,
    facts: &SpecFacts,
    ty: TypeId,
    spec_symbol: etas_hir::SymbolId,
    args: &[TypeId],
    visited: &mut HashSet<(TypeId, etas_hir::SymbolId)>,
) -> bool {
    if !visited.insert((ty, spec_symbol)) {
        return false;
    }
    if type_param_bound_satisfies_spec(store, facts, ty, spec_symbol, args) {
        return true;
    }
    if let Some(path) = facts.std_spec_aliases.get(&spec_symbol)
        && std_type_satisfies_spec(store, facts, ty, path, args)
    {
        return true;
    }
    facts.impls.iter().any(|impl_fact| {
        type_same(store, impl_fact.self_type, ty)
            && spec_entails(
                store,
                facts,
                impl_fact.spec_symbol,
                &impl_fact.args,
                spec_symbol,
                args,
                &mut HashSet::new(),
            )
    })
}

fn std_type_satisfies_spec(
    store: &TypeStore,
    facts: &SpecFacts,
    ty: TypeId,
    spec: &[String],
    args: &[TypeId],
) -> bool {
    if std_type_param_bound_satisfies_spec(store, facts, ty, spec, args) {
        return true;
    }
    facts.std_impls.iter().any(|implementation| {
        implementation.spec == spec
            && type_same(store, implementation.self_type, ty)
            && args_match(store, &implementation.args, args)
    })
}

fn std_type_param_bound_satisfies_spec(
    store: &TypeStore,
    facts: &SpecFacts,
    ty: TypeId,
    spec: &[String],
    args: &[TypeId],
) -> bool {
    let Some(Type::Named(NamedTypeRef { name })) = store.get(ty) else {
        return false;
    };
    facts
        .type_param_bounds
        .values()
        .flatten()
        .filter(|bound| bound.param_name == *name)
        .any(|bound| {
            facts
                .std_spec_aliases
                .get(&bound.spec_symbol)
                .is_some_and(|path| path == spec && args_match(store, &bound.args, args))
        })
}

fn type_param_bound_satisfies_spec(
    store: &TypeStore,
    facts: &SpecFacts,
    ty: TypeId,
    spec_symbol: etas_hir::SymbolId,
    args: &[TypeId],
) -> bool {
    let Some(Type::Named(NamedTypeRef { name })) = store.get(ty) else {
        return false;
    };
    facts
        .type_param_bounds
        .values()
        .flatten()
        .filter(|bound| bound.param_name == *name)
        .any(|bound| {
            spec_entails(
                store,
                facts,
                bound.spec_symbol,
                &bound.args,
                spec_symbol,
                args,
                &mut HashSet::new(),
            )
        })
}

fn spec_entails(
    store: &TypeStore,
    facts: &SpecFacts,
    current_spec: etas_hir::SymbolId,
    current_args: &[TypeId],
    target_spec: etas_hir::SymbolId,
    target_args: &[TypeId],
    visited: &mut HashSet<etas_hir::SymbolId>,
) -> bool {
    if !visited.insert(current_spec) {
        return false;
    }
    if current_spec == target_spec && args_match(store, current_args, target_args) {
        return true;
    }

    let Some(signature) = facts.signatures.get(&current_spec) else {
        return false;
    };
    let param_substitutions = signature
        .param_names
        .iter()
        .cloned()
        .zip(current_args.iter().copied())
        .collect::<HashMap<_, _>>();

    signature.super_specs.iter().any(|super_spec| {
        let super_args = super_spec
            .args
            .iter()
            .copied()
            .map(|arg| resolve_named(store, arg, &param_substitutions))
            .collect::<Vec<_>>();
        spec_entails(
            store,
            facts,
            super_spec.super_spec_symbol,
            &super_args,
            target_spec,
            target_args,
            visited,
        )
    })
}

fn args_match(store: &TypeStore, actual: &[TypeId], expected: &[TypeId]) -> bool {
    actual.len() == expected.len()
        && actual
            .iter()
            .copied()
            .zip(expected.iter().copied())
            .all(|(actual, expected)| arg_matches(store, actual, expected))
}

fn arg_matches(store: &TypeStore, actual: TypeId, expected: TypeId) -> bool {
    if type_same(store, actual, expected) {
        return true;
    }
    matches!(
        store.get(expected),
        Some(Type::Named(NamedTypeRef { name })) if is_schematic_type_param(name)
    )
}

fn type_same(store: &TypeStore, lhs: TypeId, rhs: TypeId) -> bool {
    if lhs == rhs {
        return true;
    }
    let mut unifier = TypeUnifier::new(store);
    unifier.unify(lhs, rhs).is_ok()
}

fn resolve_named(store: &TypeStore, ty: TypeId, substitutions: &HashMap<String, TypeId>) -> TypeId {
    match store.get(ty) {
        Some(Type::Named(NamedTypeRef { name })) => substitutions.get(name).copied().unwrap_or(ty),
        _ => ty,
    }
}

fn spec_name(facts: &SpecFacts, symbol: etas_hir::SymbolId) -> String {
    facts
        .signatures
        .get(&symbol)
        .map(|signature| signature.name.clone())
        .unwrap_or_else(|| format!("symbol{}", symbol.0))
}

fn checked_spec_name(facts: &SpecFacts, spec: &CheckedSpecRef) -> String {
    match spec {
        CheckedSpecRef::Source(symbol) => spec_name(facts, *symbol),
        CheckedSpecRef::Std(path) => path.join("."),
    }
}

fn display_type_name(store: &TypeStore, ty: TypeId) -> String {
    match store.get(ty) {
        Some(Type::Named(name)) => name.name.clone(),
        Some(Type::Nominal(nominal)) => nominal.name.clone(),
        Some(Type::Primitive(primitive)) => format!("{primitive:?}"),
        _ => format!("type{}", ty.0),
    }
}

fn is_schematic_type_param(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}
