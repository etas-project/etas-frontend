use etas_hir::HirStmt;

use crate::{
    AssignabilityReason, ConstraintOrigin, PrimitiveType, TypeConstraint, TypeId,
    lower::type_ref::lower_type_ref,
    pipeline::{
        body::collect::{block::collect_block, expr::collect_expr, pattern::collect_pattern},
        context::BodyCollectContext,
    },
};

pub fn collect_stmt(ctx: &mut BodyCollectContext<'_, '_>, stmt: etas_hir::HirStmtId) -> TypeId {
    let unit = ctx.primitive(PrimitiveType::Unit);
    let ty = match ctx.ctx.hir.stmts[stmt].clone() {
        HirStmt::Let {
            pat,
            type_annotation,
            value,
            span,
        }
        | HirStmt::Var {
            pat,
            type_annotation,
            value,
            span,
        } => {
            let expected = type_annotation.and_then(|ty| lower_type_ref(ctx.ctx, ty));
            let actual = collect_expr(ctx, value, expected);
            let binding_ty = expected.unwrap_or(actual);
            if let Some(expected) = expected {
                ctx.emit(TypeConstraint::Assignable {
                    from: actual,
                    to: expected,
                    origin: ConstraintOrigin { span },
                    reason: AssignabilityReason::Annotation,
                });
            }
            collect_pattern(ctx, pat, binding_ty);
            unit
        }
        HirStmt::Return { value, span } => {
            if ctx.state.handler_depth > 0 {
                ctx.validate(crate::ValidationRequest::Diagnostic {
                    code: etas_core::TypeDiagnosticCode::ReturnInsideHandlerArm,
                    span,
                    message: "return is not valid inside a handler arm; use finish to complete the handled expression".to_owned(),
                });
            }
            let actual = value
                .map(|value| collect_expr(ctx, value, ctx.state.expected_return))
                .unwrap_or(unit);
            if let Some(expected) = ctx.state.expected_return {
                ctx.emit(TypeConstraint::Assignable {
                    from: actual,
                    to: expected,
                    origin: ConstraintOrigin { span },
                    reason: AssignabilityReason::Return,
                });
            }
            ctx.primitive(PrimitiveType::Never)
        }
        HirStmt::Expr { expr, .. } => collect_expr(ctx, expr, None),
        HirStmt::If(expr) => {
            match ctx.ctx.hir.exprs[expr].clone() {
                etas_hir::HirExpr::If {
                    cond,
                    then_block,
                    else_branch,
                    span,
                } => crate::pipeline::body::collect::expr::collect_if_statement(
                    ctx,
                    cond,
                    then_block,
                    else_branch,
                    span,
                ),
                _ => {
                    collect_expr(ctx, expr, None);
                }
            }
            unit
        }
        HirStmt::Match(expr) => {
            match ctx.ctx.hir.exprs[expr].clone() {
                etas_hir::HirExpr::Match {
                    scrutinee, arms, ..
                } => crate::pipeline::body::collect::expr::collect_match_statement(
                    ctx, scrutinee, arms,
                ),
                _ => {
                    collect_expr(ctx, expr, None);
                }
            }
            unit
        }
        HirStmt::Assign {
            target,
            value,
            span,
        } => {
            let target_ty = collect_expr(ctx, target, None);
            let value_ty = collect_expr(ctx, value, Some(target_ty));
            ctx.emit(TypeConstraint::Assignable {
                from: value_ty,
                to: target_ty,
                origin: ConstraintOrigin { span },
                reason: AssignabilityReason::Assignment,
            });
            unit
        }
        HirStmt::For {
            pat,
            iter,
            limits,
            body,
            span,
        } => {
            for limit in limits {
                collect_expr(ctx, limit, None);
            }
            let elem = ctx.fresh_type_var();
            let iter_ty = collect_expr(ctx, iter, None);
            let key = ctx.fresh_type_var();
            let value = ctx.fresh_type_var();
            let entry_pair = ctx
                .ctx
                .interner
                .intern(crate::Type::Tuple(vec![key, value]));
            ctx.emit(TypeConstraint::Iterable {
                iter: iter_ty,
                item: elem,
                entry_pair,
                origin: ConstraintOrigin { span },
            });
            collect_pattern(ctx, pat, elem);
            collect_block(ctx, body, Some(unit));
            unit
        }
        HirStmt::While {
            cond,
            limits,
            body,
            span,
        } => {
            for limit in limits {
                collect_expr(ctx, limit, None);
            }
            let bool_ty = ctx.primitive(PrimitiveType::Bool);
            let cond_ty = collect_expr(ctx, cond, Some(bool_ty));
            ctx.emit(TypeConstraint::Assignable {
                from: cond_ty,
                to: bool_ty,
                origin: ConstraintOrigin { span },
                reason: AssignabilityReason::Other,
            });
            collect_block(ctx, body, Some(unit));
            unit
        }
        HirStmt::Retry { limits, body, .. } => {
            for limit in limits {
                collect_expr(ctx, limit, None);
            }
            collect_block(ctx, body, Some(unit));
            unit
        }
        HirStmt::Resume { value, span } => {
            let expected = ctx.state.handler_resume_stack.last().copied();
            let actual = value
                .map(|value| collect_expr(ctx, value, expected))
                .unwrap_or(unit);
            if let Some(expected) = expected {
                if matches!(
                    ctx.ctx.interner.store().get(expected),
                    Some(crate::Type::Primitive(PrimitiveType::Never))
                ) {
                    ctx.validate(crate::ValidationRequest::Diagnostic {
                        code: etas_core::TypeDiagnosticCode::TypeMismatch,
                        span,
                        message: "cannot resume a handled action whose return type is never"
                            .to_owned(),
                    });
                }
                ctx.emit(TypeConstraint::Assignable {
                    from: actual,
                    to: expected,
                    origin: ConstraintOrigin { span },
                    reason: AssignabilityReason::HandlerResume,
                });
            }
            unit
        }
        HirStmt::Finish { value, span } => {
            let expected = ctx.state.handler_finish_stack.last().copied();
            let actual = collect_expr(ctx, value, expected);
            if ctx.state.handler_depth == 0 {
                ctx.validate(crate::ValidationRequest::Diagnostic {
                    code: etas_core::TypeDiagnosticCode::FinishOutsideHandler,
                    span,
                    message: "finish is only valid inside a handler arm".to_owned(),
                });
            } else if let Some(expected) = expected {
                ctx.emit(TypeConstraint::Assignable {
                    from: actual,
                    to: expected,
                    origin: ConstraintOrigin { span },
                    reason: AssignabilityReason::Finish,
                });
            }
            ctx.primitive(PrimitiveType::Never)
        }
        HirStmt::Break { .. } | HirStmt::Continue { .. } => unit,
        HirStmt::Error { span } => {
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span,
                message: "error statement cannot be typed".to_owned(),
            });
            unit
        }
    };
    ctx.record_stmt_type(stmt, ty)
}
