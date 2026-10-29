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
        .map(|collected| collected.ty)
}

pub(super) struct CollectedHandler {
    pub ty: TypeId,
    pub finishes: Vec<(TypeId, crate::ConstraintOrigin)>,
}

pub(super) fn collect_handler_for_requests<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    handlers: &[HirHandlerArmId],
    span: etas_core::Span,
    expected: Option<TypeId>,
    requests: &[super::handler_request::ActionRequest],
    request_body: Option<etas_hir::HirExprId>,
) -> Result<CollectedHandler, E> {
    ctx.checkpoint()?;
    let mut handled = Vec::new();
    let mut finishes = Vec::new();
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
            etas_hir::ResolveResult::Resolved(symbol) => {
                action_signature(ctx, symbol, *handler, &arm, requests, request_body)
            }
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
        validate_handler_arm_completion(ctx, arm.body)?;
        let resume_ty = action_signature
            .as_ref()
            .map(|signature| signature.output)
            .unwrap_or_else(|| ctx.fresh_type_var());
        ctx.state.handler_resume_stack.push(resume_ty);
        ctx.state.handler_finish_stack.push(expected_result);
        ctx.state.handler_finish_candidates.push(Vec::new());
        ctx.state.handler_depth += 1;
        let collected = collect_block(ctx, arm.body, None);
        ctx.state.handler_depth -= 1;
        if let Some(candidates) = ctx.state.handler_finish_candidates.pop() {
            finishes.extend(candidates);
        } else {
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span: arm.span,
                message: "handler completion constraints lost their lexical collection scope"
                    .into(),
            });
        }
        ctx.state.handler_finish_stack.pop();
        ctx.state.handler_resume_stack.pop();
        collected?;
    }
    let result = expected_result.or_else(|| (!finishes.is_empty()).then(|| ctx.fresh_type_var()));
    if expected_result.is_none()
        && let Some(output) = result
    {
        for (from, origin) in &finishes {
            ctx.emit(crate::TypeConstraint::Assignable {
                from: *from,
                to: output,
                origin: *origin,
                reason: crate::AssignabilityReason::Finish,
            });
        }
        let never = ctx.primitive(crate::PrimitiveType::Never);
        ctx.emit(crate::TypeConstraint::ControlResult {
            output,
            candidates: finishes.iter().map(|(ty, _)| *ty).collect(),
            never,
            origin: crate::ConstraintOrigin { span },
        });
    }
    let ty = ctx.ctx.interner.intern(Type::Handler(HandlerType {
        handled: expected_handled.unwrap_or(EffectRowRef {
            effects: handled,
            tail: None,
        }),
        produced: expected_produced,
        result,
    }));
    Ok(CollectedHandler { ty, finishes })
}

fn action_signature<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    symbol: etas_hir::SymbolId,
    handler: HirHandlerArmId,
    arm: &etas_hir::HirHandlerArm,
    requests: &[super::handler_request::ActionRequest],
    request_body: Option<etas_hir::HirExprId>,
) -> Option<crate::EffectActionSignature> {
    let action = super::handler_request::action_identity(ctx, symbol);
    let scope = request_body.map(|body| (handler, body));
    if let Some((_, signatures)) = super::action_selector::overload_signatures(ctx, symbol) {
        let signatures = signatures
            .into_iter()
            .map(|(_, signature)| {
                let signature =
                    specialize_action_signature(ctx, signature, &arm.action.effect.args, arm.span)?;
                specialize_action_signature_from_generic_args(
                    ctx,
                    signature,
                    &arm.generic_args,
                    arm.span,
                )
            })
            .collect::<Option<Vec<_>>>()?;
        return super::handler_request::instantiate_overloads(
            ctx, signatures, requests, action, scope, arm.span,
        );
    }
    let signature = ctx
        .state
        .provisional
        .symbol_types
        .get(&symbol)
        .or_else(|| ctx.ctx.symbol_fact(symbol))
        .and_then(|fact| match fact {
            SymbolTypeFact::EffectAction { signature } => Some(signature.clone()),
            _ => None,
        })?;
    let signature = specialize_action_signature(ctx, signature, &arm.action.effect.args, arm.span)?;
    let signature =
        specialize_action_signature_from_generic_args(ctx, signature, &arm.generic_args, arm.span)?;
    super::handler_request::instantiate(ctx, signature, action, requests, scope, arm.span)
}

fn validate_handler_arm_completion<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    block: etas_hir::HirBlockId,
) -> Result<(), E> {
    ctx.checkpoint()?;
    let block_data = ctx.ctx.hir.blocks[block].clone();
    if let Some(final_expr) = block_data.final_expr {
        let span = ctx.ctx.hir.exprs[final_expr].span(&ctx.ctx.hir.blocks);
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: etas_core::TypeDiagnosticCode::HandlerCompletionRequired,
            span,
            message: "handler arm must explicitly resume, finish, or terminate with never"
                .to_owned(),
        });
        return Ok(());
    }
    let Some(last_stmt) = block_data.stmts.last().copied() else {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: etas_core::TypeDiagnosticCode::HandlerCompletionRequired,
            span: block_data.span,
            message: "handler arm must explicitly resume, finish, or terminate with never"
                .to_owned(),
        });
        return Ok(());
    };
    match ctx.ctx.hir.stmts[last_stmt].clone() {
        HirStmt::Resume { .. } | HirStmt::Finish { .. } => {}
        HirStmt::Return { span, .. } => {
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::ReturnInsideHandlerArm,
                span,
                message: "return is not valid inside a handler arm; use finish to complete the handled expression".to_owned(),
            });
        }
        HirStmt::Expr { expr, span } => {
            if !is_obvious_never_expr(ctx, expr) {
                ctx.validate(crate::ValidationRequest::Diagnostic {
                    code: etas_core::TypeDiagnosticCode::HandlerCompletionRequired,
                    span,
                    message: "handler arm must explicitly resume, finish, or terminate with never"
                        .to_owned(),
                });
            }
        }
        _ => {
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::HandlerCompletionRequired,
                span: block_data.span,
                message: "handler arm must explicitly resume, finish, or terminate with never"
                    .to_owned(),
            });
        }
    }
    Ok(())
}

fn is_obvious_never_expr<E>(
    ctx: &BodyCollectContext<'_, '_, E>,
    expr: etas_hir::HirExprId,
) -> bool {
    let etas_hir::HirExpr::Call { callee, .. } = &ctx.ctx.hir.exprs[expr] else {
        return false;
    };
    let etas_hir::HirExpr::Path(path) = &ctx.ctx.hir.exprs[*callee] else {
        return false;
    };
    path.segments
        .last()
        .is_some_and(|segment| segment.name == "abort")
}
