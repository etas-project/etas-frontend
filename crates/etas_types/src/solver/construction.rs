use super::SolverFailure;
use crate::{ConstraintOrigin, KnownStdTypes, Type, TypeId, TypeStore};

pub(super) fn validate(
    store: &TypeStore,
    known: &KnownStdTypes,
    target: TypeId,
    origin: ConstraintOrigin,
) -> Option<SolverFailure> {
    let target = match store.get(target) {
        Some(Type::Applied { constructor, .. }) => TypeId(constructor.0),
        _ => target,
    };
    known
        .intrinsic_only_constructors
        .contains(&target)
        .then(|| SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: origin.span,
            message: "this standard type can only be created by its registered backend operations"
                .into(),
        })
}
