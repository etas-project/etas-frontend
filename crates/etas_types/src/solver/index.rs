use crate::{
    CheckedIndexKind, CheckedSliceKind, ConstraintOrigin, PrimitiveType, Type, TypeId, TypeStore,
    solver::{
        Substitution, TypeUnifier,
        report::{SolverFailure, SolverReport},
    },
};

pub struct IndexAccessSolveInput<'a> {
    pub spec_facts: &'a crate::SpecFacts,
    pub expr: etas_hir::HirExprId,
    pub base: TypeId,
    pub index: TypeId,
    pub output: TypeId,
    pub index_error: Option<TypeId>,
    pub origin: ConstraintOrigin,
    pub substitutions: &'a Substitution,
}

#[derive(Clone, Copy)]
enum IndexShape {
    Sequence { value: TypeId },
    Map { key: TypeId, value: TypeId },
}

fn index_shape(
    store: &TypeStore,
    base: TypeId,
    origin: ConstraintOrigin,
) -> Result<IndexShape, SolverFailure> {
    match store.get(base) {
        Some(Type::Array(value)) | Some(Type::List(value)) | Some(Type::Slice(value)) => {
            Ok(IndexShape::Sequence { value: *value })
        }
        Some(Type::Primitive(primitive @ (PrimitiveType::String | PrimitiveType::Bytes))) => {
            let element = match primitive {
                PrimitiveType::String => PrimitiveType::Char,
                _ => PrimitiveType::U8,
            };
            let value = primitive_type_id(store, element).ok_or_else(|| SolverFailure {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span: origin.span,
                message: format!(
                    "missing primitive type for indexed {} output",
                    element.source_name()
                ),
            })?;
            Ok(IndexShape::Sequence { value })
        }
        Some(Type::Map { key, value }) => Ok(IndexShape::Map {
            key: *key,
            value: *value,
        }),
        _ => Err(SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: origin.span,
            message: "value is not indexable".to_owned(),
        }),
    }
}

// Propagate container types before defaulting literals, without publishing a
// checked access until its index and Index evidence have also been validated.
pub(super) fn infer_index_access(
    store: &TypeStore,
    input: &IndexAccessSolveInput<'_>,
) -> SolverReport {
    let mut report = SolverReport::default();
    let base = resolve_substitution(store, input.substitutions, input.base);
    let shape = match index_shape(store, base, input.origin) {
        Ok(shape) => shape,
        Err(failure) => {
            report.push(failure);
            return report;
        }
    };
    let mut unifier = TypeUnifier::with_known_substitution(store, input.substitutions);
    let value = match shape {
        IndexShape::Sequence { value } => value,
        IndexShape::Map { key, value } => {
            let index = resolve_substitution(store, input.substitutions, input.index);
            let key = resolve_substitution(store, input.substitutions, key);
            unify_or_assign(store, index, key, input.origin, &mut unifier, &mut report);
            value
        }
    };
    let value = resolve_substitution(store, input.substitutions, value);
    let output = resolve_substitution(store, input.substitutions, input.output);
    unify_or_assign(
        store,
        value,
        output,
        input.origin,
        &mut unifier,
        &mut report,
    );
    if report.failures.is_empty() {
        report.substitutions.extend(&unifier.into_substitution());
    }
    report
}

pub fn solve_index_access(store: &TypeStore, input: IndexAccessSolveInput<'_>) -> SolverReport {
    let mut report = infer_index_access(store, &input);
    if !report.failures.is_empty() {
        return report;
    }
    let IndexAccessSolveInput {
        spec_facts,
        expr,
        base,
        index,
        output,
        index_error,
        origin,
        substitutions,
    } = input;
    let base = resolve_substitution(store, substitutions, base);
    let index = resolve_substitution(store, substitutions, index);
    let output = resolve_substitution(store, substitutions, output);
    let shape = match index_shape(store, base, origin) {
        Ok(shape) => shape,
        Err(failure) => {
            report.push(failure);
            return report;
        }
    };
    match shape {
        IndexShape::Sequence { .. } => {
            if !is_index_type(store, spec_facts, index) {
                report.push(SolverFailure {
                    code: etas_core::TypeDiagnosticCode::TypeMismatch,
                    span: origin.span,
                    message: "sequence index must satisfy Index".to_owned(),
                });
            }
            if !report.failures.is_empty() {
                return report;
            }
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
        IndexShape::Map { key, value } => {
            let key = resolve_substitution(store, substitutions, key);
            let value = resolve_substitution(store, substitutions, value);
            report.record_index_kind(expr, CheckedIndexKind::MapLookup { key, value });
        }
    }
    report
}

pub struct SliceAccessSolveInput<'a> {
    pub spec_facts: &'a crate::SpecFacts,
    pub expr: etas_hir::HirExprId,
    pub base: TypeId,
    pub start: TypeId,
    pub end: TypeId,
    pub output: TypeId,
    pub origin: ConstraintOrigin,
    pub substitutions: &'a Substitution,
}

pub fn solve_slice_access(store: &TypeStore, input: SliceAccessSolveInput<'_>) -> SolverReport {
    let SliceAccessSolveInput {
        spec_facts,
        expr,
        base,
        start,
        end,
        output,
        origin,
        substitutions,
    } = input;
    let mut report = SolverReport::default();
    let base = resolve_substitution(store, substitutions, base);
    let start = resolve_substitution(store, substitutions, start);
    let end = resolve_substitution(store, substitutions, end);
    let output = resolve_substitution(store, substitutions, output);
    let mut unifier = TypeUnifier::with_known_substitution(store, substitutions);
    if !is_index_type(store, spec_facts, start) || !is_index_type(store, spec_facts, end) {
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
            unify_or_assign(store, slice, output, origin, &mut unifier, &mut report);
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
            unify_or_assign(store, range, output, origin, &mut unifier, &mut report);
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
    if report.failures.is_empty() {
        report.substitutions.extend(&unifier.into_substitution());
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
    unifier: &mut TypeUnifier<'_>,
    report: &mut SolverReport,
) {
    if matches!(
        store.get(actual),
        Some(Type::Primitive(PrimitiveType::Never))
    ) {
        return;
    }
    if unifier.unify(actual, expected).is_err() {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: origin.span,
            message: "indexed value type does not match expected output".to_owned(),
        });
    }
}

fn is_index_type(store: &TypeStore, facts: &crate::SpecFacts, ty: TypeId) -> bool {
    let spec = etas_std::StdSupportConstraint::Index
        .spec_path()
        .iter()
        .map(|segment| (*segment).to_owned())
        .collect::<Vec<_>>();
    matches!(store.get(ty), Some(Type::IntegerLiteral { .. }))
        || super::spec_solver::std_type_satisfies_spec(store, facts, ty, &spec, &[])
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
