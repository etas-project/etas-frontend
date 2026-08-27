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
    let record = match record_fields(store, base) {
        Ok(Some(record)) => record,
        Ok(None) => {
            report.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::UnknownField,
                span: origin.span,
                message: format!("cannot access field `{field}` on this value"),
            });
            return report;
        }
        Err(error) => {
            report.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span: origin.span,
                message: format!("record representation substitution failed: {error}"),
            });
            return report;
        }
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

fn record_fields(
    store: &TypeStore,
    ty: TypeId,
) -> Result<Option<crate::RecordType>, crate::TypeSubstitutionError> {
    let Some(ty_data) = store.get(ty) else {
        return Err(crate::TypeSubstitutionError::MissingType(ty));
    };
    match ty_data {
        Type::Record(record) => Ok(Some(record.clone())),
        Type::MemoryRegion(schema) => record_fields(store, *schema),
        Type::ResourceHandle(ResourceHandleType::MemoryRegion { schema }) => {
            record_fields(store, *schema)
        }
        Type::Nominal(_) | Type::Applied { .. } => {
            crate::record_fields_with_applied_params(store, ty)
        }
        _ => Ok(None),
    }
}
