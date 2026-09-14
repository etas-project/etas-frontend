use super::{SolverFailure, SolverReport, TypeUnifier, assignability, resolve_known_substitutions};
use crate::{ConstraintOrigin, Type, TypeId, TypeStore};

pub(super) fn solve(
    store: &TypeStore,
    report: &mut SolverReport,
    iter: TypeId,
    item: TypeId,
    entry_pair: TypeId,
    origin: &ConstraintOrigin,
) {
    let iter = resolve_known_substitutions(store, report, iter);
    let item = resolve_known_substitutions(store, report, item);
    let mut unifier = TypeUnifier::with_known_substitution(store, &report.substitutions);
    let item_type = match store.get(iter) {
        Some(
            Type::Array(inner)
            | Type::List(inner)
            | Type::Set(inner)
            | Type::Slice(inner)
            | Type::MemorySelection(inner),
        ) => *inner,
        Some(Type::Range { index }) => *index,
        Some(Type::Map { key, value }) => {
            let Some(Type::Tuple(fields)) = store.get(entry_pair) else {
                report.push(SolverFailure {
                    code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                    span: origin.span,
                    message: "map iterable constraint is missing its entry pair shape".into(),
                });
                return;
            };
            let [entry_key, entry_value] = fields.as_slice() else {
                report.push(SolverFailure {
                    code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                    span: origin.span,
                    message: "map iterable entry shape must have exactly two fields".into(),
                });
                return;
            };
            if unifier.unify(*entry_key, *key).is_err()
                || unifier.unify(*entry_value, *value).is_err()
            {
                report.push(mismatch(origin));
                return;
            }
            entry_pair
        }
        _ => {
            report.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::TypeMismatch,
                span: origin.span,
                message: "for-loop iterator must be an iterable collection or range".into(),
            });
            return;
        }
    };
    if unifier.unify(item_type, item).is_ok() {
        report.substitutions.extend(&unifier.into_substitution());
    } else if !assignability::is_assignable(store, item_type, item) {
        report.push(mismatch(origin));
    }
}

fn mismatch(origin: &ConstraintOrigin) -> SolverFailure {
    SolverFailure {
        code: etas_core::TypeDiagnosticCode::TypeMismatch,
        span: origin.span,
        message: "for-loop iterator item type does not match pattern".into(),
    }
}
