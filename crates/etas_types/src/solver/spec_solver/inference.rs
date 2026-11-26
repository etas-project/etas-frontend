use std::collections::HashSet;

use crate::solver::{
    report::{SolverFailure, SolverReport},
    unification::Substitution,
};
use crate::{SpecFacts, SpecObligation, Type, TypeId, TypeStore, TypeUnifier};

// A proof can solve arguments of a bound (C ~ Control<Live>). Retain that
// unification in the body solver before publishing call instantiations.
pub(in crate::solver) fn infer_obligation_arguments(
    store: &TypeStore,
    facts: &SpecFacts,
    obligations: &[SpecObligation],
    known: &Substitution,
    finalize: bool,
) -> SolverReport {
    let mut report = SolverReport::default();
    if !obligations
        .iter()
        .flat_map(|obligation| &obligation.args)
        .any(|arg| unresolved(store, known, *arg))
    {
        return report;
    }
    let mut solved = known.clone();
    loop {
        let mut changed = false;
        for obligation in obligations {
            let receiver = super::super::resolve_substitution(store, &solved, obligation.ty);
            if unresolved(store, &solved, receiver)
                || !obligation
                    .args
                    .iter()
                    .any(|arg| unresolved(store, &solved, *arg))
            {
                continue;
            }
            let arguments =
                match super::bound_evidence::arguments(store, facts, receiver, &obligation.spec) {
                    Ok(arguments) => arguments,
                    Err(message) => {
                        report.push(SolverFailure {
                            code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                            span: obligation.span,
                            message,
                        });
                        return report;
                    }
                };
            let mut selected = None;
            let mut ambiguous = false;
            for arguments in arguments {
                if arguments.len() != obligation.args.len() {
                    continue;
                }
                let mut unifier = TypeUnifier::with_known_substitution(store, &solved);
                if !obligation
                    .args
                    .iter()
                    .zip(arguments)
                    .all(|(expected, actual)| unifier.unify(*expected, actual).is_ok())
                {
                    continue;
                }
                if selected.is_some() {
                    ambiguous = true;
                    break;
                }
                selected = Some(unifier.into_substitution());
            }
            if !ambiguous
                && let Some(bindings) = selected
                && !bindings.is_empty()
            {
                solved.extend(&bindings);
                report.substitutions.extend(&bindings);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for obligation in obligations {
        if finalize
            && obligation
                .args
                .iter()
                .any(|arg| unresolved(store, &solved, *arg))
        {
            report.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::TypeMismatch,
                span: obligation.span,
                message: format!(
                    "type arguments for spec bound `{}` could not be inferred uniquely; specify the generic arguments",
                    super::checked_spec_name(facts, &obligation.spec),
                ),
            });
        }
    }
    report
}

fn unresolved(store: &TypeStore, bindings: &Substitution, ty: TypeId) -> bool {
    let mut pending = vec![ty];
    let mut seen = HashSet::new();
    while let Some(ty) = pending.pop() {
        let ty = super::super::resolve_substitution(store, bindings, ty);
        if !seen.insert(ty) {
            continue;
        }
        match store.get(ty) {
            Some(Type::Var(_)) => return true,
            Some(value) => pending.extend(crate::type_dependencies(value)),
            None => {}
        }
    }
    false
}
