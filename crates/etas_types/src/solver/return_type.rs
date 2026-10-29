use crate::{
    PrimitiveType, SolverFailure, SolverReport, Type, TypeConstraint, TypeStore, TypeUnifier,
};

pub(super) fn solve<E>(
    store: &TypeStore,
    report: &mut SolverReport,
    constraints: &[&TypeConstraint],
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<(), E> {
    for constraint in constraints {
        checkpoint()?;
        if let TypeConstraint::ControlResult {
            output,
            candidates,
            never,
            origin,
        } = constraint
        {
            let output = super::resolve_known_substitutions(store, report, *output);
            if !candidates.is_empty()
                && matches!(store.get(output), Some(Type::Var(_)))
                && candidates.iter().all(|candidate| {
                    matches!(
                        store.get(super::resolve_known_substitutions(
                            store, report, *candidate
                        )),
                        Some(Type::Primitive(PrimitiveType::Never))
                    )
                })
            {
                let mut unifier =
                    TypeUnifier::with_known_substitution(store, &report.substitutions);
                match unifier.unify(output, *never) {
                    Ok(()) => report.substitutions.extend(&unifier.into_substitution()),
                    Err(error) => report.push(SolverFailure {
                        code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                        span: origin.span,
                        message: format!(
                            "unreachable control result could not be completed: {error:?}"
                        ),
                    }),
                }
            }
            continue;
        }
        let TypeConstraint::ReturnType {
            output,
            candidates,
            empty,
            origin,
        } = constraint
        else {
            continue;
        };
        let mut constrained = false;
        for (candidate, candidate_origin) in candidates {
            checkpoint()?;
            let candidate = super::resolve_known_substitutions(store, report, *candidate);
            if matches!(
                store.get(candidate),
                Some(Type::Primitive(PrimitiveType::Never))
            ) {
                continue;
            }
            constrained = true;
            let mut unifier = TypeUnifier::with_known_substitution(store, &report.substitutions);
            if unifier.unify(*output, candidate).is_ok() {
                report.substitutions.extend(&unifier.into_substitution());
            } else {
                report.push(SolverFailure {
                    code: etas_core::TypeDiagnosticCode::TypeMismatch,
                    span: candidate_origin.span,
                    message: "return candidates must have one consistent inferred type".into(),
                });
            }
        }
        if !constrained {
            let mut unifier = TypeUnifier::with_known_substitution(store, &report.substitutions);
            if unifier.unify(*output, *empty).is_ok() {
                report.substitutions.extend(&unifier.into_substitution());
            } else {
                report.push(SolverFailure {
                    code: etas_core::TypeDiagnosticCode::TypeMismatch,
                    span: origin.span,
                    message: "flow without a return candidate must have unit output".into(),
                });
            }
        }
    }
    Ok(())
}
