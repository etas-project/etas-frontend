use etas_hir::{
    HirEffectArg, HirHandlerArmId, HirStmt, PartialResolutionReason, ResolveResult, SymbolDef,
    TopLevelLetClassification,
};

use crate::{
    EffectArgRef, EffectRef, EffectRowRef, HandlerProducedEffects, HandlerType, SymbolTypeFact,
    Type, TypeId,
    pipeline::{
        body::collect::{
            action_selector::{
                specialize_action_signature, validate_action_selector_arity_and_kinds,
            },
            block::collect_block,
            pattern::collect_pattern,
        },
        context::BodyCollectContext,
    },
};

pub fn collect_handler(
    ctx: &mut BodyCollectContext<'_, '_>,
    handlers: &[HirHandlerArmId],
    span: etas_core::Span,
    expected: Option<TypeId>,
) -> TypeId {
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
        let arm = ctx.ctx.hir.handler_arms[*handler].clone();
        let action_signature = match arm.action.action_symbol {
            etas_hir::ResolveResult::Resolved(symbol) => ctx
                .state
                .provisional
                .symbol_types
                .get(&symbol)
                .or_else(|| {
                    ctx.ctx
                        .symbols
                        .symbol_fact(ctx.ctx.hir, &ctx.ctx.signature_facts, symbol)
                })
                .and_then(|fact| match fact {
                    SymbolTypeFact::EffectAction { signature } => Some(signature.clone()),
                    _ => None,
                })
                .map(|signature| {
                    specialize_action_signature(ctx, signature, &arm.action.effect.args)
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
        if let Some(args) = arm
            .action
            .effect
            .args
            .iter()
            .map(|arg| handler_effect_arg(ctx, arg))
            .collect::<Option<Vec<_>>>()
        {
            handled.push(EffectRef {
                name: arm
                    .action
                    .effect
                    .path
                    .segments
                    .iter()
                    .map(|segment| segment.name.as_str())
                    .collect::<Vec<_>>()
                    .join("."),
                args,
            });
        }
        if let Some(signature) = &action_signature {
            for (pat, expected) in arm
                .patterns
                .iter()
                .copied()
                .zip(signature.params.iter().copied())
            {
                collect_pattern(ctx, pat, expected);
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
                collect_pattern(ctx, pat, ty);
            }
        }
        validate_handler_arm_completion(ctx, arm.body);
        let resume_ty = action_signature
            .as_ref()
            .map(|signature| signature.output)
            .unwrap_or_else(|| ctx.fresh_type_var());
        ctx.state.handler_resume_stack.push(resume_ty);
        if let Some(result) = expected_result {
            ctx.state.handler_finish_stack.push(result);
        }
        ctx.state.handler_depth += 1;
        collect_block(ctx, arm.body, None);
        ctx.state.handler_depth -= 1;
        if expected_result.is_some() {
            ctx.state.handler_finish_stack.pop();
        }
        ctx.state.handler_resume_stack.pop();
    }
    ctx.ctx.interner.intern(Type::Handler(HandlerType {
        handled: expected_handled.unwrap_or(EffectRowRef {
            effects: handled,
            tail: None,
        }),
        produced: expected_produced,
        result: expected_result,
    }))
}

fn handler_effect_arg(
    ctx: &mut BodyCollectContext<'_, '_>,
    arg: &HirEffectArg,
) -> Option<EffectArgRef> {
    match arg {
        HirEffectArg::Type(ty) => {
            let lowered = crate::lower::type_ref::lower_type_ref(ctx.ctx, *ty);
            if lowered.is_none()
                && let Some(span) = ctx.ctx.hir.types.get(*ty).map(|ty| ty.span())
            {
                invalid_static_handler_effect_arg(ctx, span);
            }
            lowered.map(EffectArgRef::Type)
        }
        HirEffectArg::Path(path) => {
            let (symbol, remaining) = match &path.resolution {
                ResolveResult::Resolved(symbol) => (*symbol, Vec::new()),
                ResolveResult::PartiallyResolved(partial)
                    if partial.reason == PartialResolutionReason::MemberRequiresTypeChecking =>
                {
                    match partial.resolved_prefix {
                        Some(symbol) => (symbol, partial.remaining.clone()),
                        None => {
                            invalid_static_handler_effect_arg(ctx, path.span);
                            return None;
                        }
                    }
                }
                _ => {
                    invalid_static_handler_effect_arg(ctx, path.span);
                    return None;
                }
            };
            match ctx.ctx.symbols.type_fact_as_type_id(
                ctx.ctx.hir,
                &ctx.ctx.signature_facts,
                symbol,
            ) {
                Some(ty) => Some(EffectArgRef::Type(ty)),
                None => match static_resource_path_segments(ctx, symbol, remaining) {
                    Some(segments) => Some(EffectArgRef::Path(segments)),
                    None => {
                        invalid_static_handler_effect_arg(ctx, path.span);
                        None
                    }
                },
            }
        }
        HirEffectArg::Wildcard { .. } => Some(EffectArgRef::Wildcard),
        HirEffectArg::String { value, .. } => Some(EffectArgRef::String(value.clone())),
        HirEffectArg::Int { text, .. } => Some(EffectArgRef::Int(text.clone())),
    }
}

fn static_resource_path_segments(
    ctx: &BodyCollectContext<'_, '_>,
    symbol: etas_hir::SymbolId,
    remaining: Vec<String>,
) -> Option<Vec<String>> {
    let symbol = ctx
        .ctx
        .symbols
        .canonical_symbol(ctx.ctx.hir, symbol)
        .unwrap_or(symbol);
    let symbol = ctx.ctx.hir.symbols.get(symbol)?;
    if let SymbolDef::ImportAlias { path, .. } = &symbol.def {
        let mut path = path.clone();
        path.extend(remaining);
        return Some(path);
    }
    let static_symbol = match &symbol.def {
        SymbolDef::Item { .. } | SymbolDef::EnumVariant { .. } | SymbolDef::Synthetic { .. } => {
            true
        }
        SymbolDef::TopLevelLet { classification, .. } => matches!(
            classification,
            TopLevelLetClassification::Unknown
                | TopLevelLetClassification::Const
                | TopLevelLetClassification::ResourceHandle(_)
                | TopLevelLetClassification::Handler
        ),
        _ => false,
    };
    if !static_symbol {
        return None;
    }
    let module = ctx.ctx.hir.modules_arena.get(symbol.defining_module)?;
    let mut segments = module
        .name
        .as_ref()
        .map(|module_name| {
            module_name
                .segments
                .iter()
                .map(|segment| segment.name.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    segments.push(symbol.name.clone());
    segments.extend(remaining);
    Some(segments)
}

fn invalid_static_handler_effect_arg(ctx: &mut BodyCollectContext<'_, '_>, span: etas_core::Span) {
    ctx.validate(crate::ValidationRequest::Diagnostic {
        code: etas_core::TypeDiagnosticCode::InvalidEffectArgument,
        span,
        message: "handler effect argument must be a static selector, literal, type argument, or `_`; runtime values belong in the action payload".to_owned(),
    });
}

fn validate_handler_arm_completion(
    ctx: &mut BodyCollectContext<'_, '_>,
    block: etas_hir::HirBlockId,
) {
    let block_data = ctx.ctx.hir.blocks[block].clone();
    if let Some(final_expr) = block_data.final_expr {
        let span = ctx.ctx.hir.exprs[final_expr].span(&ctx.ctx.hir.blocks);
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: etas_core::TypeDiagnosticCode::HandlerCompletionRequired,
            span,
            message: "handler arm must explicitly resume, finish, or terminate with never"
                .to_owned(),
        });
        return;
    }
    let Some(last_stmt) = block_data.stmts.last().copied() else {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: etas_core::TypeDiagnosticCode::HandlerCompletionRequired,
            span: block_data.span,
            message: "handler arm must explicitly resume, finish, or terminate with never"
                .to_owned(),
        });
        return;
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
}

fn is_obvious_never_expr(ctx: &BodyCollectContext<'_, '_>, expr: etas_hir::HirExprId) -> bool {
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
