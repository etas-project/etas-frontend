use etas_hir::HirLiteral;

use crate::{
    ConstraintOrigin, NumericLiteralKind, PrimitiveType, Type, TypeConstraint, TypeId,
    pipeline::context::BodyCollectContext,
};

pub(super) fn trusted_constructor<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    callee: etas_hir::HirExprId,
    explicit: &[TypeId],
) -> Option<TypeId> {
    let registry = ctx.ctx.std_registry.clone();
    let target = super::std_member::resolved_std_callee(ctx, &registry, callee)?;
    let identity = registry.declaration_key(target.id)?;
    if identity.module() != ["std", "security", "trust"] || identity.declaration() != ["Trusted"] {
        return None;
    }
    let string = ctx.primitive(PrimitiveType::String);
    if !explicit.is_empty() && explicit != [string] {
        return None;
    }
    let owner = crate::lower::std::lower_std_type_symbol(ctx.ctx, &registry, target)?;
    Some(ctx.ctx.interner.intern(Type::Applied {
        constructor: crate::TypeConstructorId(owner.0),
        args: vec![string],
    }))
}

pub fn collect_literal<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    expr: Option<etas_hir::HirExprId>,
    literal: &HirLiteral,
    expected: Option<TypeId>,
) -> TypeId {
    match literal {
        HirLiteral::Bool { .. } => ctx.primitive(PrimitiveType::Bool),
        HirLiteral::Int { text, span } => {
            let ty = numeric_expected(ctx, expected, NumericLiteralKind::Integer)
                .unwrap_or_else(|| ctx.fresh_type_var());
            ctx.emit(TypeConstraint::NumericLiteral {
                expr,
                text: text.clone(),
                kind: NumericLiteralKind::Integer,
                ty,
                origin: ConstraintOrigin { span: *span },
            });
            ty
        }
        HirLiteral::Float { text, span } => {
            let ty = numeric_expected(ctx, expected, NumericLiteralKind::Float)
                .unwrap_or_else(|| ctx.fresh_type_var());
            ctx.emit(TypeConstraint::NumericLiteral {
                expr,
                text: text.clone(),
                kind: NumericLiteralKind::Float,
                ty,
                origin: ConstraintOrigin { span: *span },
            });
            ty
        }
        HirLiteral::String { .. } => ctx.primitive(PrimitiveType::String),
        HirLiteral::Char { .. } => ctx.primitive(PrimitiveType::Char),
    }
}

pub fn collect_negated_numeric_literal<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    expr: etas_hir::HirExprId,
    literal: &HirLiteral,
    expected: Option<TypeId>,
    span: etas_core::Span,
) -> Option<TypeId> {
    let (text, kind) = match literal {
        HirLiteral::Int { text, .. } => (format!("-{text}"), NumericLiteralKind::Integer),
        HirLiteral::Float { text, .. } => (format!("-{text}"), NumericLiteralKind::Float),
        _ => return None,
    };
    let ty = numeric_expected(ctx, expected, kind).unwrap_or_else(|| ctx.fresh_type_var());
    ctx.emit(TypeConstraint::NumericLiteral {
        expr: Some(expr),
        text,
        kind,
        ty,
        origin: ConstraintOrigin { span },
    });
    Some(ctx.record_expr_type(expr, ty))
}

fn numeric_expected<E>(
    ctx: &BodyCollectContext<'_, '_, E>,
    expected: Option<TypeId>,
    kind: NumericLiteralKind,
) -> Option<TypeId> {
    expected.filter(|ty| {
        matches!(
            (kind, ctx.ctx.interner.store().get(*ty)),
            (_, Some(Type::Var(_)))
                | (
                    NumericLiteralKind::Integer,
                    Some(Type::Primitive(
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
                            | PrimitiveType::USize,
                    )),
                )
                | (
                    NumericLiteralKind::Float,
                    Some(Type::Primitive(PrimitiveType::F32 | PrimitiveType::F64)),
                )
        )
    })
}
