use std::collections::HashSet;

use etas_hir::{HirFieldInit, HirGenericArg, HirRecordExpr, ResolveResult};

use crate::{
    AssignabilityReason, ConstraintOrigin, FieldType, RecordType, SymbolTypeFact, Type,
    TypeConstraint, TypeConstructorId, TypeId,
    lower::type_ref::lower_type_ref,
    pipeline::{body::collect::expr::collect_expr, context::BodyCollectContext},
};

pub fn collect_record(
    ctx: &mut BodyCollectContext<'_, '_>,
    record: &HirRecordExpr,
    expected: Option<TypeId>,
) -> TypeId {
    let constructor_target = record
        .path
        .as_ref()
        .and_then(|path| record_constructor_type(ctx, path, &record.generic_args, record.span));
    let field_hint_target = constructor_target.or(expected);
    let target_fields = field_hint_target.and_then(|ty| record_fields(ctx, ty));
    let mut seen = HashSet::new();
    let mut fields = Vec::new();
    for field in &record.fields {
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
        let ty = field_value(field)
            .map(|value| collect_expr(ctx, value, expected_field))
            .unwrap_or_else(|| ctx.fresh_type_var());
        fields.push(FieldType { name, ty });
    }
    let actual = ctx.ctx.interner.intern(Type::Record(RecordType { fields }));
    if let Some(expected) = constructor_target {
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
    }
}

fn record_constructor_type(
    ctx: &mut BodyCollectContext<'_, '_>,
    path: &etas_hir::ResolvedPath,
    generic_args: &[HirGenericArg],
    span: etas_core::Span,
) -> Option<TypeId> {
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return None;
    };
    let args = lower_record_generic_args(ctx, generic_args);
    let fact = ctx
        .ctx
        .symbols
        .symbol_fact(ctx.ctx.hir, &ctx.ctx.signature_facts, symbol)?
        .clone();
    match fact {
        SymbolTypeFact::TypeAlias { target, params } => {
            Some(apply_alias_type_args(ctx, target, params, args, span))
        }
        SymbolTypeFact::Type { constructor } | SymbolTypeFact::NominalType { constructor, .. } => {
            Some(apply_constructor_type_args(
                ctx,
                TypeId(constructor.0),
                args,
            ))
        }
        _ => ctx
            .ctx
            .symbols
            .type_fact_as_type_id(ctx.ctx.hir, &ctx.ctx.signature_facts, symbol),
    }
}

fn lower_record_generic_args(
    ctx: &mut BodyCollectContext<'_, '_>,
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

fn apply_alias_type_args(
    ctx: &mut BodyCollectContext<'_, '_>,
    target: TypeId,
    params: Vec<String>,
    args: Vec<TypeId>,
    span: etas_core::Span,
) -> TypeId {
    if params.len() != args.len() {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: etas_core::TypeDiagnosticCode::ArityMismatch,
            span,
            message: "record constructor type alias was given the wrong number of type arguments"
                .to_owned(),
        });
        return ctx.primitive(crate::PrimitiveType::Never);
    }
    if params.is_empty() {
        return target;
    }
    let substitutions = params.into_iter().zip(args).collect();
    crate::substitute_named_params(&mut ctx.ctx.interner, target, &substitutions)
}

fn apply_constructor_type_args(
    ctx: &mut BodyCollectContext<'_, '_>,
    base: TypeId,
    args: Vec<TypeId>,
) -> TypeId {
    if args.is_empty() {
        base
    } else {
        ctx.ctx.interner.intern(Type::Applied {
            constructor: TypeConstructorId(base.0),
            args,
        })
    }
}

fn field_name(field: &HirFieldInit) -> &str {
    match field {
        HirFieldInit::Shorthand { name, .. } | HirFieldInit::Named { name, .. } => name,
    }
}

fn field_value(field: &HirFieldInit) -> Option<etas_hir::HirExprId> {
    match field {
        HirFieldInit::Named { value, .. } => Some(*value),
        HirFieldInit::Shorthand {
            resolution: ResolveResult::Resolved(_),
            ..
        } => None,
        HirFieldInit::Shorthand { .. } => None,
    }
}

fn field_span(field: &HirFieldInit) -> etas_core::Span {
    match field {
        HirFieldInit::Shorthand { span, .. } | HirFieldInit::Named { span, .. } => *span,
    }
}

fn record_fields(ctx: &mut BodyCollectContext<'_, '_>, ty: TypeId) -> Option<RecordType> {
    let ty = crate::applied_representation(&mut ctx.ctx.interner, ty).unwrap_or(ty);
    match ctx.ctx.interner.store().get(ty)? {
        Type::Record(record) => Some(record.clone()),
        _ => None,
    }
}

fn is_direct_record_type(ctx: &BodyCollectContext<'_, '_>, ty: TypeId) -> bool {
    matches!(ctx.ctx.interner.store().get(ty), Some(Type::Record(_)))
}
