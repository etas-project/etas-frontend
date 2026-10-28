use std::collections::HashMap;

use etas_hir::{HirExpr, HirExprId, ResolveResult, SymbolDef, SymbolId};

use crate::{
    CallableGenericParamKind, ConstraintOrigin, EffectActionSignature, Type, TypeConstraint,
    TypeId, pipeline::context::BodyCollectContext,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::pipeline) enum ActionIdentity {
    Source(SymbolId),
    Standard(etas_std::StdSymbolId),
}

pub(in crate::pipeline) enum ActionRequest {
    Known {
        action: ActionIdentity,
        call: HirExprId,
    },
    Opaque {
        call: Option<HirExprId>,
    },
}

pub(super) fn collect_handle<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    body: HirExprId,
    handler: HirExprId,
    span: etas_core::Span,
    expected: Option<TypeId>,
) -> Result<TypeId, E> {
    let start = ctx.state.action_requests.len();
    ctx.state.request_scope_depth += 1;
    let body_ty = super::expr::collect_expr(ctx, body, expected);
    ctx.state.request_scope_depth -= 1;
    let body_ty = body_ty?;
    let requests = ctx.state.action_requests.drain(start..).collect::<Vec<_>>();
    let handler_ty = if let HirExpr::Handler { handlers, span } = ctx.ctx.hir.exprs[handler].clone()
    {
        let ty = super::handler::collect_handler_for_requests(
            ctx,
            &handlers,
            span,
            None,
            &requests,
            Some(body),
        )?;
        let targets = handlers
            .iter()
            .filter_map(|arm| {
                let arm = &ctx.ctx.hir.handler_arms[*arm];
                if !arm
                    .generic_args
                    .iter()
                    .all(|arg| matches!(arg, etas_hir::HirGenericArg::Wildcard { .. }))
                    || !arm
                        .action
                        .effect
                        .args
                        .iter()
                        .all(|arg| matches!(arg, etas_hir::HirEffectArg::Wildcard { .. }))
                    || !arm.patterns.iter().all(|pat| {
                        matches!(
                            ctx.ctx.hir.pats[*pat],
                            etas_hir::HirPat::Binding { .. } | etas_hir::HirPat::Wildcard { .. }
                        )
                    })
                {
                    return None;
                }
                match arm.action.action_symbol {
                    ResolveResult::Resolved(symbol) => Some(action_identity(ctx, symbol)),
                    _ => None,
                }
            })
            .collect::<Vec<_>>();
        ctx.state
            .action_requests
            .extend(requests.into_iter().filter(|request| match request {
                ActionRequest::Known { action, .. } => !targets.contains(action),
                ActionRequest::Opaque { .. } => true,
            }));
        ctx.record_expr_type(handler, ty)
    } else {
        ctx.state.action_requests.extend(requests);
        super::expr::collect_expr(ctx, handler, None)?
    };
    if !matches!(
        ctx.ctx.interner.store().get(handler_ty),
        Some(Type::Handler(_))
    ) {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span,
            message: "handle expects a handler value".to_owned(),
        });
    }
    Ok(body_ty)
}

pub(super) fn record_request<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    action: ActionIdentity,
    call: HirExprId,
) {
    if ctx.state.request_scope_depth == 0 {
        return;
    }
    remove_call(ctx, call);
    ctx.state
        .action_requests
        .push(ActionRequest::Known { action, call });
}

fn remove_call<E>(ctx: &mut BodyCollectContext<'_, '_, E>, call: HirExprId) {
    if let Some(index) = ctx.state.action_requests.iter().rposition(|request| {
        matches!(request,
            ActionRequest::Opaque { call: Some(candidate) } if *candidate == call
        )
    }) {
        ctx.state.action_requests.remove(index);
    }
}

pub(super) fn action_identity<E>(
    ctx: &BodyCollectContext<'_, '_, E>,
    symbol: SymbolId,
) -> ActionIdentity {
    let symbol = ctx
        .ctx
        .symbols
        .canonical_symbol(ctx.ctx.hir, symbol)
        .unwrap_or(symbol);
    if let Some(target) = ctx.ctx.hir.standard_action_targets.get(&symbol) {
        return ActionIdentity::Standard(*target);
    }
    if let Some(SymbolDef::ImportAlias { path, .. }) =
        ctx.ctx.hir.symbols.get(symbol).map(|symbol| &symbol.def)
        && let Some(target) = ctx.ctx.std_registry.lookup_qualified(path)
        && matches!(target.decl, etas_std::StdDecl::EffectAction(_))
    {
        return ActionIdentity::Standard(target.id);
    }
    ActionIdentity::Source(symbol)
}

pub(super) fn record_forwarded_call<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    call: HirExprId,
    callee: HirExprId,
) {
    // Only a checked registry forwarder proves that its generic binders and
    // payload/result ABI are those of the action. Source calls need a request
    // summary; a public escaping row alone cannot prove the requested actions.
    let standard = match &ctx.ctx.hir.exprs[callee] {
        HirExpr::Path(path) => match path.resolution {
            ResolveResult::Resolved(symbol) => {
                let symbol = ctx
                    .ctx
                    .symbols
                    .canonical_symbol(ctx.ctx.hir, symbol)
                    .unwrap_or(symbol);
                match ctx.ctx.hir.symbols.get(symbol).map(|symbol| &symbol.def) {
                    Some(SymbolDef::ImportAlias { path, .. }) => {
                        ctx.ctx.std_registry.lookup_qualified(path)
                    }
                    _ => None,
                }
            }
            _ => None,
        },
        _ => None,
    };
    if let Some(symbol) = standard {
        if let Some(etas_std::IntrinsicDescriptor {
            lowering: etas_std::LoweringHint::ForwardAction { stable_id },
            ..
        }) = &symbol.intrinsic
        {
            let action = ctx.ctx.std_registry.symbols().find(|symbol| matches!(
                &symbol.decl, etas_std::StdDecl::EffectAction(action) if action.stable_id == Some(*stable_id)
            ));
            if let Some(action) = action {
                let action = ActionIdentity::Standard(action.id);
                record_request(ctx, action, call);
                return;
            }
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span: ctx.ctx.hir.exprs[call].span(&ctx.ctx.hir.blocks),
                message: "checked action forwarder has no registered action target".into(),
            });
            return;
        }
        if symbol.intrinsic.as_ref().is_some_and(|intrinsic| {
            intrinsic.purity == etas_std::IntrinsicPurity::Pure
                && intrinsic.latent_effect == etas_std::IntrinsicLatentEffect::None
        }) {
            remove_call(ctx, call);
        }
    }
}

pub(super) fn instantiate<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    signature: EffectActionSignature,
    action: ActionIdentity,
    requests: &[ActionRequest],
    scope: Option<(etas_hir::HirHandlerArmId, HirExprId)>,
    span: etas_core::Span,
) -> Option<EffectActionSignature> {
    let parameters = signature
        .generic_params
        .iter()
        .filter(|parameter| parameter.kind == CallableGenericParamKind::Type)
        .filter(|parameter| {
            !signature
                .selector_param_names
                .iter()
                .zip(&signature.selector_defaults)
                .any(|(name, value)| {
                    name == &parameter.name && matches!(value, Some(crate::EffectArgRef::Type(_)))
                })
        })
        .map(|parameter| {
            (
                parameter.name.clone(),
                ctx.fresh_type_var(),
                parameter.subject,
            )
        })
        .collect::<Vec<_>>();
    if parameters.is_empty() {
        return Some(signature);
    }
    let opaque = requests
        .iter()
        .any(|request| matches!(request, ActionRequest::Opaque { .. }));
    let request_calls = if opaque {
        Vec::new()
    } else {
        requests
            .iter()
            .filter_map(|request| match request {
                ActionRequest::Known {
                    action: target,
                    call,
                } if *target == action => Some(*call),
                _ => None,
            })
            .collect()
    };
    let bindings = parameters
        .iter()
        .map(|(name, variable, _)| (name.clone(), *variable))
        .collect::<HashMap<_, _>>();
    let defaults = signature.selector_defaults.clone();
    let signature =
        super::action_selector::apply_substitutions(ctx, signature, defaults, &bindings, span)?;
    ctx.emit(TypeConstraint::HandlerInstantiation {
        parameters,
        requests: request_calls,
        deferred: scope.filter(|_| opaque),
        origin: ConstraintOrigin { span },
    });
    Some(signature)
}
