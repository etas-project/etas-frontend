use crate::{
    AssignabilityReason, ConstraintOrigin, PrimitiveType, SolverFailure, SolverReport, Type,
    TypeId, TypeStore, TypeUnifier,
};

#[derive(Clone, Copy)]
pub(super) struct Discard {
    pub value: TypeId,
    pub output: TypeId,
    pub unit: TypeId,
    pub origin: ConstraintOrigin,
}

pub(super) fn solve_ready<E>(
    store: &TypeStore,
    report: &mut SolverReport,
    pending: &mut Vec<Discard>,
    finalize: bool,
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<(), E> {
    loop {
        let previous = pending.len();
        let requests = std::mem::take(pending);
        for request in requests {
            checkpoint()?;
            if !solve_one(store, report, request, finalize) {
                pending.push(request);
            }
        }
        if pending.len() == previous {
            break;
        }
    }
    if finalize {
        for request in pending.drain(..) {
            checkpoint()?;
            report.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span: request.origin.span,
                message: "discarded expression has no solved operand type".into(),
            });
        }
    }
    Ok(())
}

fn solve_one(
    store: &TypeStore,
    report: &mut SolverReport,
    request: Discard,
    finalize: bool,
) -> bool {
    if !matches!(
        store.get(request.unit),
        Some(Type::Primitive(PrimitiveType::Unit))
    ) || store.get(request.output).is_none()
    {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
            span: request.origin.span,
            message: "discard constraint is missing its unit or result type".into(),
        });
        return true;
    }
    let value = super::resolve_known_substitutions(store, report, request.value);
    if super::is_unresolved_type_var(store, value) {
        return false;
    }
    let Some(value_type) = store.get(value) else {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
            span: request.origin.span,
            message: "discarded expression references a missing type".into(),
        });
        return true;
    };
    let output = super::resolve_known_substitutions(store, report, request.output);
    let diverges = matches!(value_type, Type::Primitive(PrimitiveType::Never));
    if diverges && super::is_unresolved_type_var(store, output) && !finalize {
        return false;
    }
    let result = if diverges { value } else { request.unit };
    if !super::is_assignable_for_reason(store, result, output, &AssignabilityReason::Other) {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: request.origin.span,
            message: "an expression statement discards its value and produces unit".into(),
        });
        return true;
    }
    let mut unifier = TypeUnifier::with_known_substitution(store, &report.substitutions);
    if unifier.unify(result, output).is_ok() {
        report.substitutions.extend(&unifier.into_substitution());
    }
    true
}
