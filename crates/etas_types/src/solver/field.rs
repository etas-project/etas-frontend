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
    known: &crate::KnownStdTypes,
    base: TypeId,
    field: &str,
    output: TypeId,
    origin: ConstraintOrigin,
) -> SolverReport {
    let mut report = SolverReport::default();
    let projected = projected_fields(store, known, base);
    let record = match projected.and_then(|projected| {
        projected.map_or_else(|| record_fields(store, base), |record| Ok(Some(record)))
    }) {
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

fn projected_fields(
    store: &TypeStore,
    known: &crate::KnownStdTypes,
    base: TypeId,
) -> Result<Option<crate::RecordType>, crate::TypeSubstitutionError> {
    let Some(fields) = known.projected_field_templates(store, base) else {
        return Ok(None);
    };
    let mut substitutions = std::collections::HashMap::new();
    if let Some(Type::Applied { constructor, args }) = store.get(base) {
        let Some(Type::Nominal(nominal)) = store.get(TypeId(constructor.0)) else {
            return Err(crate::TypeSubstitutionError::MissingType(TypeId(
                constructor.0,
            )));
        };
        if nominal.params.len() != args.len() {
            return Err(crate::TypeSubstitutionError::InvalidApplication(base));
        }
        substitutions.extend(nominal.params.iter().cloned().zip(args.iter().copied()));
    }
    let mut record = crate::RecordType {
        fields: fields.to_vec(),
    };
    for field in &mut record.fields {
        field.ty = crate::substitute_named_params_in_store(store, field.ty, &substitutions)?;
    }
    Ok(Some(record))
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
