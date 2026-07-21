use crate::{
    ConstraintOrigin, ResourceHandleType, Type, TypeId, TypeStore,
    solver::{
        TypeUnifier,
        assignability::is_assignable,
        report::{SolverFailure, SolverReport},
    },
};

pub fn solve_field_access(
    store: &TypeStore,
    base: TypeId,
    field: &str,
    output: TypeId,
    origin: ConstraintOrigin,
) -> SolverReport {
    let mut report = SolverReport::default();
    let Some(record) = record_fields(store, base) else {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::UnknownField,
            span: origin.span,
            message: format!("cannot access field `{field}` on this value"),
        });
        return report;
    };
    let Some(found) = record
        .fields
        .iter()
        .find(|candidate| candidate.name == field)
    else {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::UnknownField,
            span: origin.span,
            message: format!("record has no field `{field}`"),
        });
        return report;
    };
    let mut unifier = TypeUnifier::new(store);
    if unifier.unify(found.ty, output).is_ok() {
        report.substitutions.extend(unifier.substitution());
    } else if !is_assignable(store, found.ty, output) {
        report.push(SolverFailure {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span: origin.span,
            message: format!("field `{field}` type does not match expected output"),
        });
    }
    report
}

fn record_fields(store: &TypeStore, ty: TypeId) -> Option<crate::RecordType> {
    match store.get(ty)? {
        Type::Record(record) => Some(record.clone()),
        Type::MemoryRegion(schema) => record_fields(store, *schema),
        Type::ResourceHandle(ResourceHandleType::MemoryRegion { schema }) => {
            record_fields(store, *schema)
        }
        Type::Nominal(_) | Type::Applied { .. } => {
            crate::record_fields_with_applied_params(store, ty)
        }
        _ => None,
    }
}
