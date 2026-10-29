use etas_hir::{HirHandlerArmId, HirStmt, ResolveResult, SymbolDef};

use crate::{
    EffectRef, EffectRowRef, HandlerProducedEffects, HandlerType, SymbolTypeFact, Type, TypeId,
    pipeline::{
        body::collect::{
            action_selector::{
                checked_selector_arguments, specialize_action_signature,
                specialize_action_signature_from_generic_args,
                validate_action_selector_arity_and_kinds,
            },
            block::collect_block,
            pattern::collect_pattern,
        },
        context::BodyCollectContext,
    },
};

pub fn collect_handler<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    handlers: &[HirHandlerArmId],
    span: etas_core::Span,
    expected: Option<TypeId>,
) -> Result<TypeId, E> {
    collect_handler_for_requests(ctx, handlers, span, expected, &[], None)
}

pub(super) fn collect_handler_for_requests<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    handlers: &[HirHandlerArmId],
    span: etas_core::Span,
    expected: Option<TypeId>,
    requests: &[super::handler_request::ActionRequest],
    request_body: Option<etas_hir::HirExprId>,
) -> Result<TypeId, E> {
    ctx.checkpoint()?;
    let mut handled = Vec::new();
    if handlers.is_empty() {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: etas_core::TypeDiagnosticCode::EmptyHandler,
            span,
            message: "handler requires at least one handler arm".to_owned(),
        });
    }
    let expected_handler = expected.and_then(|ty| match ctx.ctx.interner.store().get(ty) {
        Some(Type::Handler(handler)) => Some(handler.clone()),
        _ => None,
    });
    let expected_result = expected_handler.as_ref().and_then(|handler| handler.result);
    let expected_produced = expected_handler
        .as_ref()
        .map(|handler| handler.produced.clone())
        .unwrap_or(HandlerProducedEffects::Infer);
    let expected_handled = expected_handler.and_then(|handler| {
        if handler.handled.effects.is_empty() {
            None
        } else {
            Some(handler.handled)
        }
    });
    for handler in handlers {
        ctx.checkpoint()?;
        let arm = ctx.ctx.hir.handler_arms[*handler].clone();
        let action_signature = match arm.action.action_symbol {
            etas_hir::ResolveResult::Resolved(symbol) => ctx
                .state
                .provisional
                .symbol_types
                .get(&symbol)
                .or_else(|| ctx.ctx.symbol_fact(symbol))
                .and_then(|fact| match fact {
                    SymbolTypeFact::EffectAction { signature } => Some(signature.clone()),
                    _ => None,
                })
                .and_then(|signature| {
                    let signature = specialize_action_signature(
                        ctx,
                        signature,
                        &arm.action.effect.args,
                        arm.span,
                    )?;
                    let signature = specialize_action_signature_from_generic_args(
                        ctx,
                        signature,
                        &arm.generic_args,
                        arm.span,
                    )?;
                    let action = super::handler_request::action_identity(ctx, symbol);
                    super::handler_request::instantiate(
                        ctx,
                        signature,
                        action,
                        requests,
                        request_body.map(|body| (*handler, body)),
                        arm.span,
                    )
                }),
            _ => None,
        };
        if let Some(signature) = &action_signature {
            validate_action_selector_arity_and_kinds(
                ctx,
                signature,
                &arm.generic_args,
                arm.span,
                "handler action",
            );
        }
        if let (Some(signature), ResolveResult::Resolved(action_symbol)) =
            (&action_signature, &arm.action.action_symbol)
        {
            let explicit_count = arm.generic_args.len();
            if let Some(selectors) = checked_selector_arguments(signature, explicit_count) {
                let Some(symbol) = ctx.ctx.hir.symbols.get(*action_symbol) else {
                    ctx.validate(crate::ValidationRequest::Diagnostic {
                        code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                        span: arm.span,
                        message: "checked handler action has no declaration symbol".into(),
                    });
                    continue;
                };
                let mut name = if matches!(symbol.def, SymbolDef::EffectAction { .. }) {
                    let Some(path) = ctx.ctx.hir.declaration_path(*action_symbol) else {
                        ctx.validate(crate::ValidationRequest::Diagnostic {
                            code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                            span: arm.span,
                            message: "source handler action has no canonical declaration path"
                                .into(),
                        });
                        continue;
                    };
                    path.join(".")
                } else {
                    symbol.name.clone()
                };
                if let ResolveResult::Resolved(owner) = arm.action.effect.path.resolution
                    && let Some(SymbolDef::ImportAlias { path, .. }) =
                        ctx.ctx.hir.symbols.get(owner).map(|symbol| &symbol.def)
                    && matches!(ctx.ctx.std_registry.lookup_qualified(path).map(|symbol| &symbol.decl),
                        Some(etas_std::StdDecl::Effect(effect)) if effect.core && effect.name == "Error")
                {
                    name = "Error".into();
                }
                handled.push(EffectRef {
                    name,
                    args: selectors.clone(),
                });
                ctx.state.provisional.handler_actions.insert(
                    *handler,
                    crate::CheckedHandlerActionFact {
                        action_symbol: *action_symbol,
                        selectors,
                        params: signature.params.clone(),
                        output: signature.output,
                    },
                );
            } else {
                ctx.validate(crate::ValidationRequest::Diagnostic {
                    code: etas_core::TypeDiagnosticCode::InvalidEffectArgument,
                    span: arm.span,
                    message: "handler selector must be a checked static argument; runtime values belong in the action payload".into(),
                });
            }
        }
        if let Some(signature) = &action_signature {
            for (pat, expected) in arm
                .patterns
                .iter()
                .copied()
                .zip(signature.params.iter().copied())
            {
                collect_pattern(ctx, pat, expected)?;
            }
            if arm.patterns.len() != signature.params.len() {
                ctx.validate(crate::ValidationRequest::Diagnostic {
                    code: etas_core::TypeDiagnosticCode::HandlerArmArityMismatch,
                    span: arm.span,
                    message: "handler pattern arity must match the handled action signature"
                        .to_owned(),
                });
            }
        } else {
            for pat in arm.patterns.iter().copied() {
                let ty = ctx.fresh_type_var();
                collect_pattern(ctx, pat, ty)?;
            }
        }
        let resume_ty = action_signature
            .as_ref()
            .map(|signature| signature.output)
            .unwrap_or_else(|| ctx.fresh_type_var());
        ctx.state.handler_resume_stack.push(resume_ty);
        if let Some(result) = expected_result {
            ctx.state.handler_finish_stack.push(result);
        }
        ctx.state.handler_depth += 1;
        let collected = collect_block(ctx, arm.body, None);
        ctx.state.handler_depth -= 1;
        if expected_result.is_some() {
            ctx.state.handler_finish_stack.pop();
        }
        ctx.state.handler_resume_stack.pop();
        collected?;
        validate_handler_arm_completion(ctx, arm.body)?;
    }
    Ok(ctx.ctx.interner.intern(Type::Handler(HandlerType {
        handled: expected_handled.unwrap_or(EffectRowRef {
            effects: handled,
            tail: None,
        }),
        produced: expected_produced,
        result: expected_result,
    })))
}

fn validate_handler_arm_completion<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    block: etas_hir::HirBlockId,
) -> Result<(), E> {
    enum Tail {
        Block(etas_hir::HirBlockId),
        Expr(etas_hir::HirExprId),
        Stmt(etas_hir::HirStmtId),
    }
    let mut pending = vec![Tail::Block(block)];
    while let Some(tail) = pending.pop() {
        ctx.checkpoint()?;
        let missing_span = match tail {
            Tail::Block(block) => {
                let block = &ctx.ctx.hir.blocks[block];
                if let Some(expr) = block.final_expr {
                    pending.push(Tail::Expr(expr));
                    continue;
                }
                if let Some(stmt) = block.stmts.last() {
                    pending.push(Tail::Stmt(*stmt));
                    continue;
                }
                Some(block.span)
            }
            Tail::Stmt(stmt) => match ctx.ctx.hir.stmts[stmt].clone() {
                HirStmt::Resume { .. } | HirStmt::Finish { .. } => None,
                HirStmt::Return { span, .. } => {
                    ctx.validate(crate::ValidationRequest::Diagnostic {
                        code: etas_core::TypeDiagnosticCode::ReturnInsideHandlerArm,
                        span,
                        message: "return is not valid inside a handler arm; use finish to complete the handled expression".to_owned(),
                    });
                    None
                }
                HirStmt::If(expr) | HirStmt::Match(expr) | HirStmt::Expr { expr, .. } => {
                    pending.push(Tail::Expr(expr));
                    None
                }
                HirStmt::Let { span, .. }
                | HirStmt::Var { span, .. }
                | HirStmt::Assign { span, .. }
                | HirStmt::For { span, .. }
                | HirStmt::While { span, .. }
                | HirStmt::Retry { span, .. }
                | HirStmt::Break { span }
                | HirStmt::Continue { span }
                | HirStmt::Error { span } => Some(span),
            },
            Tail::Expr(expr) => match ctx.ctx.hir.exprs[expr].clone() {
                etas_hir::HirExpr::Block(block) => {
                    pending.push(Tail::Block(block));
                    None
                }
                etas_hir::HirExpr::If {
                    then_block,
                    else_branch,
                    span,
                    ..
                } => {
                    pending.push(Tail::Block(then_block));
                    match else_branch {
                        Some(etas_hir::HirElseBranch::If(expr)) => pending.push(Tail::Expr(expr)),
                        Some(etas_hir::HirElseBranch::Block(block)) => {
                            pending.push(Tail::Block(block))
                        }
                        None => {
                            ctx.validate(crate::ValidationRequest::Diagnostic {
                                code: etas_core::TypeDiagnosticCode::HandlerCompletionRequired,
                                span,
                                message: "handler arm has a branch that does not resume, finish, or terminate with never".into(),
                            });
                        }
                    }
                    None
                }
                etas_hir::HirExpr::Match { arms, span, .. } => {
                    let empty = arms.is_empty();
                    for arm in arms {
                        pending.push(match arm.body {
                            etas_hir::HirMatchArmBody::Expr(expr) => Tail::Expr(expr),
                            etas_hir::HirMatchArmBody::Block(block) => Tail::Block(block),
                        });
                    }
                    empty.then_some(span)
                }
                _ => {
                    let span = ctx.ctx.hir.exprs[expr].span(&ctx.ctx.hir.blocks);
                    if let Some(ty) = ctx.state.provisional.expr_types.get(&expr).copied() {
                        ctx.validate(crate::ValidationRequest::HandlerNeverCompletion { ty, span });
                    } else {
                        ctx.validate(crate::ValidationRequest::Diagnostic {
                            code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                            span,
                            message: "handler completion requires a collected expression type"
                                .into(),
                        });
                    }
                    None
                }
            },
        };
        if let Some(span) = missing_span {
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::HandlerCompletionRequired,
                span,
                message: "handler arm must explicitly resume, finish, or terminate with never"
                    .to_owned(),
            });
        }
    }
    Ok(())
}
