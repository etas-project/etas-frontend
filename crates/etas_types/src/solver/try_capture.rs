use std::collections::{HashMap, HashSet};

use crate::{
    AssignabilityReason, ConstraintOrigin, EffectArgRef, TypeConstraint, TypeId, TypeStore,
    TypeSubstitutionEngine,
};

use super::{SolverFailure, SolverReport, TypeUnifier, resolve_known_substitutions};

pub(super) fn solve<E>(
    store: &TypeStore,
    report: &mut SolverReport,
    constraints: &[&TypeConstraint],
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<(), E> {
    for constraint in constraints {
        checkpoint()?;
        let TypeConstraint::TryOperand {
            source,
            operand,
            output,
            error,
            origin,
        } = constraint
        else {
            continue;
        };
        let operand = resolve_known_substitutions(store, report, *operand);
        let output = resolve_known_substitutions(store, report, *output);
        if !super::is_assignable_for_reason(store, operand, output, &AssignabilityReason::Other) {
            fail(
                report,
                *origin,
                "try operand value does not match Result ok type",
            );
            continue;
        }
        unify(
            store,
            report,
            operand,
            output,
            *origin,
            "try operand value does not match Result ok type",
        );

        // This is type information from a checked invocation, not a second
        // effect inference pass. Effects still proves the actual unique escape.
        let Some(row) = report.invocation_effects.get(source).cloned() else {
            continue;
        };
        let instantiation = report.generic_instantiations.get(source);
        let types = instantiation
            .map(|fact| fact.type_bindings.iter().cloned().collect())
            .unwrap_or_default();
        let rows: HashMap<_, _> = instantiation
            .map(|fact| fact.effect_row_bindings.iter().cloned().collect())
            .unwrap_or_default();
        let mut effects = row.effects;
        // Bindings are simultaneous: a replacement's tail belongs to the
        // caller, even when it has the same spelling as the callee parameter.
        let closed = match row.tail {
            None => true,
            Some(name) => match rows.get(&name) {
                Some(row) => {
                    effects.extend(row.effects.iter().cloned());
                    row.tail.is_none()
                }
                None => false,
            },
        };
        if !closed {
            continue;
        }
        let mut substitution =
            TypeSubstitutionEngine::existing_with_variables(store, &types, &report.substitutions);
        let mut errors = HashSet::new();
        let mut invalid = None;
        for effect in effects.iter().filter(|effect| effect.name == "Error") {
            checkpoint()?;
            let [EffectArgRef::Type(ty)] = effect.args.as_slice() else {
                invalid = Some("captured Error effect requires a checked type argument".to_owned());
                break;
            };
            match substitution.substitute(*ty) {
                Ok(ty) => {
                    errors.insert(ty);
                }
                Err(error) => {
                    invalid = Some(format!(
                        "captured error type could not be specialized: {error}"
                    ));
                    break;
                }
            }
        }
        if let Some(message) = invalid {
            report.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span: origin.span,
                message,
            });
        } else if let Some(captured) = errors.iter().copied().next().filter(|_| errors.len() == 1) {
            let target = resolve_known_substitutions(store, report, *error);
            unify(
                store,
                report,
                captured,
                target,
                *origin,
                "captured Error type does not match Result error type",
            );
        }
    }
    Ok(())
}

fn unify(
    store: &TypeStore,
    report: &mut SolverReport,
    from: TypeId,
    to: TypeId,
    origin: ConstraintOrigin,
    message: &str,
) {
    let mut unifier = TypeUnifier::with_known_substitution(store, &report.substitutions);
    if unifier.unify(from, to).is_ok() {
        report.substitutions.extend(&unifier.into_substitution());
    } else {
        fail(report, origin, message);
    }
}

fn fail(report: &mut SolverReport, origin: ConstraintOrigin, message: &str) {
    report.push(SolverFailure {
        code: etas_core::TypeDiagnosticCode::TypeMismatch,
        span: origin.span,
        message: message.into(),
    });
}
