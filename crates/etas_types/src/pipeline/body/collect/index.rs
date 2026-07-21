use crate::{
    ConstraintOrigin, Type, TypeConstraint, TypeId,
    pipeline::{body::collect::expr::collect_expr, context::BodyCollectContext},
};

pub fn collect_index(
    ctx: &mut BodyCollectContext<'_, '_>,
    expr: etas_hir::HirExprId,
    base: etas_hir::HirExprId,
    index: etas_hir::HirExprId,
    span: etas_core::Span,
    expected: Option<TypeId>,
) -> TypeId {
    let base_ty = collect_expr(ctx, base, None);
    let expected_index = match ctx.ctx.interner.store().get(base_ty) {
        Some(Type::Map { key, .. }) => Some(*key),
        _ => None,
    };
    let index_ty = collect_expr(ctx, index, expected_index);
    let inferred_output = match ctx.ctx.interner.store().get(base_ty).cloned() {
        Some(Type::Array(inner)) | Some(Type::List(inner)) | Some(Type::Slice(inner)) => {
            Some(inner)
        }
        Some(Type::Primitive(crate::PrimitiveType::String)) => {
            Some(ctx.primitive(crate::PrimitiveType::Char))
        }
        Some(Type::Primitive(crate::PrimitiveType::Bytes)) => {
            Some(ctx.primitive(crate::PrimitiveType::U8))
        }
        Some(Type::Map { value, .. }) => Some(value),
        _ => None,
    };
    let output = expected
        .or(inferred_output)
        .unwrap_or_else(|| ctx.fresh_type_var());
    let index_error = checked_index_error_type(ctx, span);
    ctx.emit(TypeConstraint::IndexAccess {
        expr,
        base: base_ty,
        index: index_ty,
        output,
        index_error,
        origin: ConstraintOrigin { span },
    });
    output
}

pub fn collect_slice(
    ctx: &mut BodyCollectContext<'_, '_>,
    expr: etas_hir::HirExprId,
    base: etas_hir::HirExprId,
    start: etas_hir::HirExprId,
    end: etas_hir::HirExprId,
    span: etas_core::Span,
    expected: Option<TypeId>,
) -> TypeId {
    let base_ty = collect_expr(ctx, base, None);
    let start_ty = collect_expr(ctx, start, None);
    let end_ty = collect_expr(ctx, end, Some(start_ty));
    let inferred_output = match ctx.ctx.interner.store().get(base_ty).cloned() {
        Some(Type::Array(inner)) | Some(Type::List(inner)) | Some(Type::Slice(inner)) => {
            Some(ctx.ctx.interner.intern(Type::Slice(inner)))
        }
        Some(Type::Range { index }) => Some(ctx.ctx.interner.intern(Type::Range { index })),
        _ => None,
    };
    let output = expected
        .or(inferred_output)
        .unwrap_or_else(|| ctx.fresh_type_var());
    ctx.emit(TypeConstraint::SliceAccess {
        expr,
        base: base_ty,
        start: start_ty,
        end: end_ty,
        output,
        origin: ConstraintOrigin { span },
    });
    output
}

fn checked_index_error_type(
    ctx: &mut BodyCollectContext<'_, '_>,
    span: etas_core::Span,
) -> Option<TypeId> {
    let found = ctx.ctx.signature_facts.known_std_types.index_error;
    if found.is_none() {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
            span,
            message: "missing checked std type fact for std.runtime.error.IndexError".to_owned(),
        });
    }
    found
}
