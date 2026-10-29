use etas_hir::{HirArg, HirGenericArg, ResolveResult, ResolvedActionRef};

use crate::{
    ConstraintOrigin, EffectActionSignature, FlowType, PrimitiveType, SymbolTypeFact, Type,
    TypeConstraint, TypeId,
    pipeline::{
        body::collect::{
            action_selector::{
                specialize_action_signature, specialize_action_signature_from_generic_args,
                validate_action_selector_arity_and_kinds,
            },
            expr::collect_expr,
        },
        context::BodyCollectContext,
    },
};

pub fn collect_perform<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    expr: etas_hir::HirExprId,
    action: &ResolvedActionRef,
    generic_args: &[HirGenericArg],
    args: &[HirArg],
    span: etas_core::Span,
    expected: Option<TypeId>,
) -> Result<TypeId, E> {
    ctx.checkpoint()?;
    if let ResolveResult::Resolved(symbol) = action.action_symbol
        && let Some((target, signatures)) = super::action_selector::overload_signatures(ctx, symbol)
    {
        let mut candidates = Vec::new();
        for (overload, signature) in signatures {
            let Some(signature) =
                specialize_action_signature(ctx, signature, &action.effect.args, span)
            else {
                continue;
            };
            validate_action_selector_arity_and_kinds(
                ctx,
                &signature,
                generic_args,
                span,
                "performed action",
            );
            let Some(signature) =
                specialize_action_signature_from_generic_args(ctx, signature, generic_args, span)
            else {
                continue;
            };
            let callable = crate::CallableSignature {
                generic_params: signature.generic_params,
                params: signature.params,
                output: signature.output,
                effects: signature.default_handler_effects,
                requested_actions: None,
            };
            if let Some(mut candidate) =
                super::expr::callable_candidate_from_signature(ctx, callable, true)
            {
                candidate.operation = crate::CallableCandidateOperation::EffectAction {
                    symbol,
                    target,
                    overload,
                };
                candidates.push(candidate);
            }
        }
        let mut argument_types = Vec::with_capacity(args.len());
        for argument in args {
            argument_types.push(collect_arg(ctx, argument, None)?);
        }
        let output = expected.unwrap_or_else(|| ctx.fresh_type_var());
        let callee = ctx.fresh_type_var();
        let probes = candidates.iter().map(|_| ctx.fresh_type_var()).collect();
        ctx.emit(TypeConstraint::OverloadedCall {
            expr,
            callee,
            candidates,
            probes,
            generic_args: Vec::new(),
            arg_exprs: super::call::call_arg_exprs(args),
            args: argument_types,
            output,
            origin: ConstraintOrigin { span },
        });
        let action = super::handler_request::action_identity(ctx, symbol);
        super::handler_request::record_request(ctx, action, expr);
        return Ok(output);
    }
    let Some(signature) = action_signature(ctx, action) else {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: etas_core::TypeDiagnosticCode::UnknownType,
            span,
            message: "performed action has no checked type signature".to_owned(),
        });
        if !generic_args.is_empty() {
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::InvalidEffectArgument,
                span,
                message: "performed action selector requires a checked action descriptor"
                    .to_owned(),
            });
        }
        for arg in args {
            collect_arg(ctx, arg, None)?;
        }
        return Ok(expected.unwrap_or_else(|| ctx.primitive(PrimitiveType::Never)));
    };
    let Some(signature) = specialize_action_signature(ctx, signature, &action.effect.args, span)
    else {
        for arg in args {
            collect_arg(ctx, arg, None)?;
        }
        return Ok(ctx.primitive(PrimitiveType::Never));
    };
    validate_action_selector_arity_and_kinds(
        ctx,
        &signature,
        generic_args,
        span,
        "performed action",
    );
    let Some(signature) =
        specialize_action_signature_from_generic_args(ctx, signature, generic_args, span)
    else {
        for arg in args {
            collect_arg(ctx, arg, None)?;
        }
        return Ok(ctx.primitive(PrimitiveType::Never));
    };

    if signature.params.len() != args.len() {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: etas_core::TypeDiagnosticCode::WrongArgumentCount,
            span,
            message: format!(
                "perform expects {} argument(s), got {}",
                signature.params.len(),
                args.len()
            ),
        });
    }

    let mut arg_types = Vec::with_capacity(args.len());
    for (index, arg) in args.iter().enumerate() {
        let expected_arg = signature.params.get(index).copied();
        let actual = collect_arg(ctx, arg, expected_arg)?;
        arg_types.push(actual);
    }

    let output = expected.unwrap_or_else(|| {
        if signature.generic_params.is_empty() {
            signature.output
        } else {
            ctx.fresh_type_var()
        }
    });
    let callee = ctx.ctx.interner.intern(Type::Function(FlowType {
        input: signature.params.clone(),
        output: signature.output,
        effects: signature.default_handler_effects.clone(),
    }));
    let explicit_generic_args = generic_args
        .iter()
        .map(|arg| match arg {
            HirGenericArg::Type(ty) => crate::lower::type_ref::lower_type_ref(ctx.ctx, *ty)
                .map(crate::CallableGenericArg::Type),
            HirGenericArg::Wildcard { .. } | HirGenericArg::EffectRow(_) => None,
        })
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default();
    ctx.emit(TypeConstraint::Callable {
        call: Some(expr),
        callee,
        generic_params: signature.generic_params,
        generic_args: explicit_generic_args,
        arg_exprs: args
            .iter()
            .map(|arg| {
                Some(match arg {
                    HirArg::Positional(expr) => *expr,
                    HirArg::Named { value, .. } => *value,
                })
            })
            .collect(),
        args: arg_types,
        output,
        origin: ConstraintOrigin { span },
    });
    if let ResolveResult::Resolved(symbol) = action.action_symbol {
        let action = super::handler_request::action_identity(ctx, symbol);
        super::handler_request::record_request(ctx, action, expr);
    }
    Ok(output)
}

fn action_signature<E>(
    ctx: &BodyCollectContext<'_, '_, E>,
    action: &ResolvedActionRef,
) -> Option<EffectActionSignature> {
    let ResolveResult::Resolved(symbol) = action.action_symbol else {
        return None;
    };
    let symbol = ctx
        .ctx
        .symbols
        .canonical_symbol(ctx.ctx.hir, symbol)
        .unwrap_or(symbol);
    ctx.state
        .provisional
        .symbol_types
        .get(&symbol)
        .or_else(|| ctx.ctx.symbol_fact(symbol))
        .and_then(|fact| match fact {
            SymbolTypeFact::EffectAction { signature } => Some(signature.clone()),
            _ => None,
        })
}

fn collect_arg<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    arg: &HirArg,
    expected: Option<TypeId>,
) -> Result<TypeId, E> {
    ctx.checkpoint()?;
    match arg {
        HirArg::Positional(expr) => collect_expr(ctx, *expr, expected),
        HirArg::Named { value, .. } => collect_expr(ctx, *value, expected),
    }
}
