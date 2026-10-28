use super::{SolverFailure, SolverReport, TypeUnifier, assignability, resolve_known_substitutions};
use crate::facts::symbols::StdIterableItemFact;
use crate::{ConstraintOrigin, Type, TypeId, TypeStore};

pub(super) fn solve(
    store: &TypeStore,
    known_std_types: &crate::KnownStdTypes,
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
        Some(Type::Applied { constructor, args })
            if known_std_types
                .iterables
                .contains_key(&TypeId(constructor.0)) =>
        {
            let fact = &known_std_types.iterables[&TypeId(constructor.0)];
            let item = if args.len() != fact.arity {
                None
            } else {
                match fact.item {
                    StdIterableItemFact::Element { parameter } => {
                        args.get(parameter).map(|ty| Ok(*ty))
                    }
                    StdIterableItemFact::Entry { key, value } => {
                        args.get(key).zip(args.get(value)).map(|(key, value)| {
                            entry_item(store, &mut unifier, *key, *value, entry_pair, origin)
                        })
                    }
                }
            };
            let Some(item) = item else {
                report.push(SolverFailure {
                    code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                    span: origin.span,
                    message: "standard iterable arguments do not match its checked declaration"
                        .into(),
                });
                return;
            };
            match item {
                Ok(item) => item,
                Err(failure) => {
                    report.push(failure);
                    return;
                }
            }
        }
        Some(Type::Map { key, value }) => {
            match entry_item(store, &mut unifier, *key, *value, entry_pair, origin) {
                Ok(item) => item,
                Err(failure) => {
                    report.push(failure);
                    return;
                }
            }
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

fn entry_item(
    store: &TypeStore,
    unifier: &mut TypeUnifier<'_>,
    key: TypeId,
    value: TypeId,
    entry_pair: TypeId,
    origin: &ConstraintOrigin,
) -> Result<TypeId, SolverFailure> {
    let Some(Type::Tuple(fields)) = store.get(entry_pair) else {
        return Err(SolverFailure {
            code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
            span: origin.span,
            message: "entry iterable constraint is missing its entry pair shape".into(),
        });
    };
    let [entry_key, entry_value] = fields.as_slice() else {
        return Err(SolverFailure {
            code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
            span: origin.span,
            message: "iterable entry shape must have exactly two fields".into(),
        });
    };
    unifier
        .unify(*entry_key, key)
        .map_err(|_| mismatch(origin))?;
    unifier
        .unify(*entry_value, value)
        .map_err(|_| mismatch(origin))?;
    Ok(entry_pair)
}

fn mismatch(origin: &ConstraintOrigin) -> SolverFailure {
    SolverFailure {
        code: etas_core::TypeDiagnosticCode::TypeMismatch,
        span: origin.span,
        message: "for-loop iterator item type does not match pattern".into(),
    }
}
