use std::collections::HashSet;

use etas_hir::{HirFieldInit, HirGenericArg, HirRecordExpr, ResolveResult};

use crate::{
    AssignabilityReason, ConstraintOrigin, FieldType, RecordType, Type, TypeConstraint, TypeId,
    lower::type_ref::lower_type_ref,
    pipeline::{body::collect::expr::collect_expr, context::BodyCollectContext},
};

pub fn collect_record<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    record: &HirRecordExpr,
    expected: Option<TypeId>,
) -> Result<TypeId, E> {
    ctx.checkpoint()?;
    if let Some(ty) = super::enum_variant::collect_record(ctx, record, expected)? {
        return Ok(ty);
    }
    let constructor_target = record
        .path
        .as_ref()
        .and_then(|path| record_constructor_type(ctx, path, &record.generic_args, record.span));
    if record.path.is_some() && constructor_target.is_none() {
        super::enum_variant::invalid(
            ctx,
            record.span,
            "record constructor is not resolved to a type or named enum variant",
        );
    }
    let field_hint_target = constructor_target.or(expected);
    let target_fields = field_hint_target.and_then(|ty| record_fields(ctx, ty, record.span));
    let mut seen = HashSet::new();
    let mut fields = Vec::new();
    for field in &record.fields {
        ctx.checkpoint()?;
        let name = field_name(field).to_owned();
        if !seen.insert(name.clone()) {
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::DuplicateField,
                span: field_span(field),
                message: "record expression initializes the same field more than once".to_owned(),
            });
        }
        let expected_field = target_fields.as_ref().and_then(|record| {
            record
                .fields
                .iter()
                .find(|candidate| candidate.name == name)
                .map(|candidate| candidate.ty)
        });
        let ty = collect_field_value(ctx, field, expected_field)?;
        fields.push(FieldType { name, ty });
    }
    let actual = ctx.ctx.interner.intern(Type::Record(RecordType { fields }));
    let ty = if let Some(expected) = constructor_target {
        ctx.emit(TypeConstraint::Constructible {
            target: expected,
            origin: ConstraintOrigin { span: record.span },
        });
        let reason = if record.path.is_some() {
            AssignabilityReason::Constructor
        } else {
            AssignabilityReason::Annotation
        };
        ctx.emit(TypeConstraint::Assignable {
            from: actual,
            to: expected,
            origin: ConstraintOrigin { span: record.span },
            reason,
        });
        expected
    } else if let Some(expected) = expected.filter(|ty| is_direct_record_type(ctx, *ty)) {
        ctx.emit(TypeConstraint::Assignable {
            from: actual,
            to: expected,
            origin: ConstraintOrigin { span: record.span },
            reason: AssignabilityReason::Annotation,
        });
        expected
    } else {
        actual
    };
    Ok(ty)
}

fn record_constructor_type<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    path: &etas_hir::ResolvedPath,
    generic_args: &[HirGenericArg],
    span: etas_core::Span,
) -> Option<TypeId> {
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return None;
    };
    let args = lower_record_generic_args(ctx, generic_args);
    let fact = ctx.ctx.symbol_fact(symbol)?.clone();
    super::nominal_constructor::target(ctx, fact, &args, span)
}

fn lower_record_generic_args<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    generic_args: &[HirGenericArg],
) -> Vec<TypeId> {
    generic_args
        .iter()
        .filter_map(|arg| match arg {
            HirGenericArg::Type(ty) => lower_type_ref(ctx.ctx, *ty),
            HirGenericArg::EffectRow(row) => {
                ctx.validate(crate::ValidationRequest::Diagnostic {
                    code: etas_core::TypeDiagnosticCode::TypeMismatch,
                    span: row.span,
                    message: "record constructor type arguments must be type arguments".to_owned(),
                });
                None
            }
            HirGenericArg::Wildcard { span } => {
                ctx.validate(crate::ValidationRequest::Diagnostic {
                    code: etas_core::TypeDiagnosticCode::TypeMismatch,
                    span: *span,
                    message: "wildcard generic argument is only valid as an effect/action selector"
                        .to_owned(),
                });
                None
            }
        })
        .collect()
}

fn field_name(field: &HirFieldInit) -> &str {
    match field {
        HirFieldInit::Shorthand { name, .. } | HirFieldInit::Named { name, .. } => name,
    }
}

pub(super) fn collect_field_value<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    field: &HirFieldInit,
    expected: Option<TypeId>,
) -> Result<TypeId, E> {
    ctx.checkpoint()?;
    match field {
        HirFieldInit::Named { value, .. } => collect_expr(ctx, *value, expected),
        HirFieldInit::Shorthand {
            resolution: ResolveResult::Resolved(symbol),
            ..
        } => Ok(
            super::expr::symbol_value_type(ctx, *symbol).unwrap_or_else(|| {
                super::enum_variant::invalid(
                    ctx,
                    field_span(field),
                    "shorthand field has no checked symbol type",
                );
                ctx.primitive(crate::PrimitiveType::Never)
            }),
        ),
        HirFieldInit::Shorthand { .. } => {
            super::enum_variant::invalid(ctx, field_span(field), "unresolved shorthand field");
            Ok(ctx.primitive(crate::PrimitiveType::Never))
        }
    }
}

fn field_span(field: &HirFieldInit) -> etas_core::Span {
    match field {
        HirFieldInit::Shorthand { span, .. } | HirFieldInit::Named { span, .. } => *span,
    }
}

fn record_fields<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    ty: TypeId,
    span: etas_core::Span,
) -> Option<RecordType> {
    let ty = match crate::applied_representation(&mut ctx.ctx.interner, ty) {
        Ok(Some(representation)) => representation,
        Ok(None) => ty,
        Err(error) => {
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span,
                message: format!("record representation substitution failed: {error}"),
            });
            return None;
        }
    };
    match ctx.ctx.interner.store().get(ty)? {
        Type::Record(record) => Some(record.clone()),
        _ => None,
    }
}

fn is_direct_record_type<E>(ctx: &BodyCollectContext<'_, '_, E>, ty: TypeId) -> bool {
    matches!(ctx.ctx.interner.store().get(ty), Some(Type::Record(_)))
}
