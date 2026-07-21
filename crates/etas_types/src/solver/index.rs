use crate::{
    CheckedIndexKind, CheckedSliceKind, ConstraintOrigin, PrimitiveType, Type, TypeId, TypeStore,
    solver::{
        Substitution, TypeUnifier,
        assignability::is_assignable,
        report::{SolverFailure, SolverReport},
    },
};

pub fn solve_index_access(
    store: &TypeStore,
    expr: etas_hir::HirExprId,
    base: TypeId,
    index: TypeId,
    output: TypeId,
    index_error: Option<TypeId>,
    origin: ConstraintOrigin,
    substitutions: &Substitution,
) -> SolverReport {
    let mut report = SolverReport::default();
    let base = resolve_substitution(store, substitutions, base);
    let index = resolve_substitution(store, substitutions, index);
    let output = resolve_substitution(store, substitutions, output);
    match store.get(base) {
        Some(Type::Array(value)) | Some(Type::List(value)) | Some(Type::Slice(value)) => {
            let value = resolve_substitution(store, substitutions, *value);
            if !is_index_type(store, index) {
                report.push(SolverFailure {
                    code: etas_core::TypeDiagnosticCode::TypeMismatch,
                    span: origin.span,
                    message: "sequence index must satisfy Index".to_owned(),
                });
            }
            unify_or_assign(store, value, output, origin, &mut report);
            report.record_index_kind(
                expr,
                CheckedIndexKind::Sequence {
                    base,
                    index,
                    output,
                },
            );
            if let Some(error) = index_error {
                report.record_checked_index_error(expr, error);
            }
        }
        Some(Type::Primitive(PrimitiveType::String)) => {
            let char_ty = primitive_type_id(store, PrimitiveType::Char).unwrap_or(output);
            if !is_index_type(store, index) {
                report.push(SolverFailure {
                    code: etas_core::TypeDiagnosticCode::TypeMismatch,
                    span: origin.span,
                    message: "string index must satisfy Index".to_owned(),
                });
            }
            unify_or_assign(store, char_ty, output, origin, &mut report);
            report.record_index_kind(
                expr,
                CheckedIndexKind::Sequence {
                    base,
                    index,
                    output,
                },
            );
            if let Some(error) = index_error {
                report.record_checked_index_error(expr, error);
            }
        }
        Some(Type::Primitive(PrimitiveType::Bytes)) => {
            let byte_ty = primitive_type_id(store, PrimitiveType::U8).unwrap_or(output);
            if !is_index_type(store, index) {
                report.push(SolverFailure {
                    code: etas_core::TypeDiagnosticCode::TypeMismatch,
                    span: origin.span,
                    message: "bytes index must satisfy Index".to_owned(),
                });
            }
            unify_or_assign(store, byte_ty, output, origin, &mut report);
            report.record_index_kind(
                expr,
                CheckedIndexKind::Sequence {
                    base,
                    index,
                    output,
                },
            );
            if let Some(error) = index_error {
                report.record_checked_index_error(expr, error);
            }
        }
        Some(Type::Map { key, value }) => {
            let key = resolve_substitution(store, substitutions, *key);
            let value = resolve_substitution(store, substitutions, *value);
            if !is_assignable(store, index, key) {
                report.push(SolverFailure {
                    code: etas_core::TypeDiagnosticCode::TypeMismatch,
                    span: origin.span,
                    message: "map index type does not match key type".to_owned(),
                });
            }
            unify_or_assign(store, value, output, origin, &mut report);
            report.record_index_kind(expr, CheckedIndexKind::MapLookup { key, value });
        }
        _ => report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: origin.span,
            message: "value is not indexable".to_owned(),
        }),
    }
    report
}

pub fn solve_slice_access(
    store: &TypeStore,
    expr: etas_hir::HirExprId,
    base: TypeId,
    start: TypeId,
    end: TypeId,
    output: TypeId,
    origin: ConstraintOrigin,
    substitutions: &Substitution,
) -> SolverReport {
    let mut report = SolverReport::default();
    let base = resolve_substitution(store, substitutions, base);
    let start = resolve_substitution(store, substitutions, start);
    let end = resolve_substitution(store, substitutions, end);
    let output = resolve_substitution(store, substitutions, output);
    if !is_index_type(store, start) || !is_index_type(store, end) {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: origin.span,
            message: "slice bounds must satisfy Index".to_owned(),
        });
    }
    match store.get(base) {
        Some(Type::Array(value)) | Some(Type::List(value)) | Some(Type::Slice(value)) => {
            let value = resolve_substitution(store, substitutions, *value);
            let slice = first_type_id(store, Type::Slice(value)).unwrap_or(output);
            unify_or_assign(store, slice, output, origin, &mut report);
            report.record_slice_kind(
                expr,
                CheckedSliceKind::Sequence {
                    base,
                    start,
                    end,
                    output,
                },
            );
        }
        Some(Type::Range { index }) => {
            let index = resolve_substitution(store, substitutions, *index);
            let range = first_type_id(store, Type::Range { index }).unwrap_or(output);
            unify_or_assign(store, range, output, origin, &mut report);
            report.record_slice_kind(
                expr,
                CheckedSliceKind::Range {
                    range: base,
                    start,
                    end,
                    output,
                },
            );
        }
        _ => report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: origin.span,
            message: "value is not sliceable".to_owned(),
        }),
    }
    report
}

fn resolve_substitution(store: &TypeStore, substitutions: &Substitution, ty: TypeId) -> TypeId {
    let mut current = ty;
    let mut seen = 0usize;
    while let Some(Type::Var(var)) = store.get(current) {
        let Some(next) = substitutions.get(*var) else {
            break;
        };
        if next == current || seen > 64 {
            break;
        }
        current = next;
        seen += 1;
    }
    current
}

fn unify_or_assign(
    store: &TypeStore,
    actual: TypeId,
    expected: TypeId,
    origin: ConstraintOrigin,
    report: &mut SolverReport,
) {
    let mut unifier = TypeUnifier::new(store);
    if unifier.unify(actual, expected).is_ok() {
        report.substitutions.extend(unifier.substitution());
    } else if !is_assignable(store, actual, expected) {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: origin.span,
            message: "indexed value type does not match expected output".to_owned(),
        });
    }
}

fn is_index_type(store: &TypeStore, ty: TypeId) -> bool {
    matches!(
        store.get(ty),
        Some(Type::IntegerLiteral { .. })
            | Some(Type::Primitive(
                PrimitiveType::I8
                    | PrimitiveType::I16
                    | PrimitiveType::I32
                    | PrimitiveType::I64
                    | PrimitiveType::I128
                    | PrimitiveType::ISize
                    | PrimitiveType::U8
                    | PrimitiveType::U16
                    | PrimitiveType::U32
                    | PrimitiveType::U64
                    | PrimitiveType::U128
                    | PrimitiveType::USize
            ))
    )
}

fn primitive_type_id(store: &TypeStore, primitive: PrimitiveType) -> Option<TypeId> {
    store.iter().find_map(|(id, ty)| {
        matches!(ty, Type::Primitive(actual) if *actual == primitive).then_some(id)
    })
}

fn first_type_id(store: &TypeStore, needle: Type) -> Option<TypeId> {
    store
        .iter()
        .find_map(|(id, ty)| (*ty == needle).then_some(id))
}
