use etas_hir::HirStmt;

use crate::{
    AssignabilityReason, ConstraintOrigin, PrimitiveType, TypeConstraint, TypeId,
    lower::type_ref::lower_type_ref,
    pipeline::{
        body::collect::{block::collect_block, expr::collect_expr, pattern::collect_pattern},
        context::BodyCollectContext,
    },
};

pub fn collect_stmt<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    stmt: etas_hir::HirStmtId,
) -> Result<TypeId, E> {
    ctx.checkpoint()?;
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
            let actual = collect_expr(ctx, value, expected)?;
            let binding_ty = expected.unwrap_or(actual);
            if let Some(expected) = expected {
                ctx.emit(TypeConstraint::Assignable {
                    from: actual,
                    to: expected,
                    origin: ConstraintOrigin { span },
                    reason: AssignabilityReason::Annotation,
                });
            }
            collect_pattern(ctx, pat, binding_ty)?;
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
                .map(|value| {
                    let expected = ctx
                        .state
                        .inferred_returns
                        .is_none()
                        .then_some(ctx.state.expected_return)
                        .flatten();
                    collect_expr(ctx, value, expected)
                })
                .transpose()?
                .unwrap_or(unit);
            if let Some(candidates) = &mut ctx.state.inferred_returns {
                candidates.push((actual, ConstraintOrigin { span }));
            } else if let Some(expected) = ctx.state.expected_return {
                ctx.emit(TypeConstraint::Assignable {
                    from: actual,
                    to: expected,
                    origin: ConstraintOrigin { span },
                    reason: AssignabilityReason::Return,
                });
            }
            ctx.primitive(PrimitiveType::Never)
        }
        HirStmt::Expr { expr, span } => {
            let value = collect_expr(ctx, expr, None)?;
            let output = ctx.fresh_type_var();
            ctx.emit(TypeConstraint::DiscardValue {
                value,
                output,
                unit,
                origin: ConstraintOrigin { span },
            });
            output
        }
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
                )?,
                _ => {
                    collect_expr(ctx, expr, None)?;
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
                )?,
                _ => {
                    collect_expr(ctx, expr, None)?;
                }
            }
            unit
        }
        HirStmt::Assign {
            target,
            value,
            span,
        } => {
            let target_ty = collect_expr(ctx, target, None)?;
            if let Some(etas_hir::HirExpr::Path(path)) = ctx.ctx.hir.exprs.get(target)
                && let etas_hir::ResolveResult::Resolved(symbol) = path.resolution
                && let Some(etas_hir::HirItem::Flow(flow)) = ctx.ctx.hir.items.get(ctx.state.item)
                && flow.receiver_mode != etas_hir::HirReceiverMode::None
                && flow.params.first() == Some(&symbol)
                && ctx
                    .ctx
                    .agent_signature(target_ty)
                    .is_some_and(|agent| agent.state.is_some())
            {
                ctx.validate(crate::ValidationRequest::Diagnostic {
                    code: etas_core::TypeDiagnosticCode::TypeMismatch,
                    span,
                    message: "an agent receiver cannot replace its instance identity".into(),
                });
            }
            ctx.validate(crate::ValidationRequest::WritablePlace { target, span });
            let value_ty = collect_expr(ctx, value, Some(target_ty))?;
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
            body,
            span,
        } => {
            if ctx.ctx.hir.source_standard_types {
                let element = ctx.fresh_type_var();
                let cursor = ctx
                    .ctx
                    .signature_base
                    .map_or(&ctx.ctx.signature_facts, |base| {
                        base.signature_facts().as_ref()
                    })
                    .known_std_types
                    .source_iterator
                    .or_else(|| {
                        crate::lower::type_ref::selected_source_type(
                            ctx.ctx,
                            &["std", "collections", "Iter"],
                        )
                    });
                if let Some(constructor) = cursor {
                    let expected = ctx.ctx.interner.intern(crate::Type::Applied {
                        constructor: crate::TypeConstructorId(constructor.0),
                        args: vec![element],
                    });
                    let layout = crate::applied_representation(&mut ctx.ctx.interner, expected);
                    let facts = ctx
                        .ctx
                        .signature_base
                        .map_or(&ctx.ctx.signature_facts, |base| {
                            base.signature_facts().as_ref()
                        });
                    match layout.and_then(|_| {
                        crate::source_iteration::checked_cursor(
                            ctx.ctx.interner.store(),
                            facts,
                            expected,
                        )
                    }) {
                        Ok(Some(_)) => {}
                        result => ctx.validate(crate::ValidationRequest::Diagnostic {
                            code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                            span,
                            message: format!(
                                "source for-loop cursor declaration is incomplete: {result:?}"
                            ),
                        }),
                    }
                    let actual = collect_expr(ctx, iter, Some(expected))?;
                    ctx.emit(TypeConstraint::Assignable {
                        from: actual,
                        to: expected,
                        origin: ConstraintOrigin { span },
                        reason: AssignabilityReason::Other,
                    });
                    collect_pattern(ctx, pat, element)?;
                    collect_block(ctx, body, Some(unit))?;
                    return Ok(ctx.record_stmt_type(stmt, unit));
                }
                ctx.validate(crate::ValidationRequest::Diagnostic {
                    code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                    span,
                    message:
                        "source for-loop requires the selected std.collections.Iter declaration"
                            .into(),
                });
                return Ok(ctx.record_stmt_type(stmt, unit));
            }
            let iter_ty = collect_expr(ctx, iter, None)?;
            let elem = super::expr::sequence_element(ctx, iter_ty, span)
                .unwrap_or_else(|| ctx.fresh_type_var());
            let key = ctx.fresh_type_var();
            let value = ctx.fresh_type_var();
            let entry_pair = ctx
                .ctx
                .interner
                .intern(crate::Type::Tuple(vec![key, value]));
            let byte_item = ctx.primitive(PrimitiveType::U8);
            ctx.emit(TypeConstraint::Iterable {
                iter: iter_ty,
                item: elem,
                entry_pair,
                byte_item,
                origin: ConstraintOrigin { span },
            });
            collect_pattern(ctx, pat, elem)?;
            collect_block(ctx, body, Some(unit))?;
            unit
        }
        HirStmt::While { cond, body, span } => {
            let bool_ty = ctx.primitive(PrimitiveType::Bool);
            let cond_ty = collect_expr(ctx, cond, Some(bool_ty))?;
            ctx.emit(TypeConstraint::Assignable {
                from: cond_ty,
                to: bool_ty,
                origin: ConstraintOrigin { span },
                reason: AssignabilityReason::Other,
            });
            collect_block(ctx, body, Some(unit))?;
            if matches!(
                ctx.ctx.hir.exprs[cond],
                etas_hir::HirExpr::Literal(etas_hir::HirLiteral::Bool { value: true, .. })
            ) && !loop_has_break(ctx.ctx.hir, body)
            {
                ctx.primitive(PrimitiveType::Never)
            } else {
                unit
            }
        }
        HirStmt::Retry { body, .. } => {
            collect_block(ctx, body, Some(unit))?;
            unit
        }
        HirStmt::Resume { value, span } => {
            let expected = ctx.state.handler_resume_stack.last().copied();
            let actual = value
                .map(|value| collect_expr(ctx, value, expected))
                .transpose()?
                .unwrap_or(unit);
            if let Some(expected) = expected {
                ctx.emit(TypeConstraint::Assignable {
                    from: actual,
                    to: expected,
                    origin: ConstraintOrigin { span },
                    reason: AssignabilityReason::HandlerResume,
                });
            }
            ctx.primitive(PrimitiveType::Never)
        }
        HirStmt::Finish { value, span } => {
            let expected = ctx.state.handler_finish_stack.last().copied().flatten();
            let actual = collect_expr(ctx, value, expected)?;
            if ctx.state.handler_depth == 0 {
                ctx.validate(crate::ValidationRequest::Diagnostic {
                    code: etas_core::TypeDiagnosticCode::FinishOutsideHandler,
                    span,
                    message: "finish is only valid inside a handler arm".to_owned(),
                });
            } else {
                if let Some(candidates) = ctx.state.handler_finish_candidates.last_mut() {
                    candidates.push((actual, ConstraintOrigin { span }));
                }
                if let Some(expected) = expected {
                    ctx.emit(TypeConstraint::Assignable {
                        from: actual,
                        to: expected,
                        origin: ConstraintOrigin { span },
                        reason: AssignabilityReason::Finish,
                    });
                }
            }
            ctx.primitive(PrimitiveType::Never)
        }
        HirStmt::Break { .. } | HirStmt::Continue { .. } => ctx.primitive(PrimitiveType::Never),
        HirStmt::Error { span } => {
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span,
                message: "error statement cannot be typed".to_owned(),
            });
            unit
        }
    };
    Ok(ctx.record_stmt_type(stmt, ty))
}

fn loop_has_break(hir: &etas_hir::HirProgram, body: etas_hir::HirBlockId) -> bool {
    use etas_hir::{HirExpr, HirTreeChild as Child};
    let mut pending = vec![Child::Block(body)];
    while let Some(child) = pending.pop() {
        match child {
            Child::Block(block) => {
                let block = &hir.blocks[block];
                pending.extend(block.stmts.iter().copied().map(Child::Stmt));
                pending.extend(block.final_expr.map(Child::Expr));
            }
            Child::Stmt(stmt) => match hir.stmts[stmt] {
                HirStmt::Break { .. } => return true,
                HirStmt::For { .. } | HirStmt::While { .. } => {}
                _ => etas_hir::for_each_stmt_child(hir, stmt, |child| pending.push(child)),
            },
            Child::Expr(expr) => {
                if !matches!(
                    hir.exprs[expr],
                    HirExpr::Lambda { .. } | HirExpr::Handler { .. }
                ) {
                    etas_hir::for_each_expr_child(hir, expr, |child| pending.push(child));
                }
            }
            Child::HandlerArm(_) | Child::Pattern(_) => {}
        }
    }
    false
}
