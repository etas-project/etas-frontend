use etas_hir::{HirArg, HirGenericArg};

use crate::{
    ConstraintOrigin, TypeConstraint, TypeId,
    pipeline::{
        body::collect::{
            call::{
                callable_output_for_expression, specialized_callable_output_for_args,
                specialized_method_output_for_args,
            },
            expr::collect_expr,
            std_member::{
                raw_std_member_value_type, raw_std_method_candidates,
                std_member_callable_signature, std_member_value_type, std_method_candidates,
            },
        },
        context::BodyCollectContext,
    },
};

pub fn collect_method_call<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    expr: etas_hir::HirExprId,
    receiver: etas_hir::HirExprId,
    method: &str,
    generic_args: &[HirGenericArg],
    args: &[HirArg],
    expected: Option<TypeId>,
) -> Result<TypeId, E> {
    ctx.checkpoint()?;
    if let Some(ty) = super::agent_method::collect_agent_method_call(
        ctx,
        expr,
        receiver,
        method,
        generic_args,
        args,
        expected,
    )? {
        return Ok(ty);
    }
    let span = ctx.ctx.hir.exprs[expr].span(&ctx.ctx.hir.blocks);
    if generic_args
        .iter()
        .any(|arg| matches!(arg, HirGenericArg::Wildcard { .. }))
    {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span,
            message: "method generic arguments cannot be wildcard selectors".to_owned(),
        });
    }
    let checked_generic_args = super::call::lower_callable_generic_args(ctx, generic_args);
    let type_generic_args = generic_args
        .iter()
        .filter_map(|arg| match arg {
            HirGenericArg::Type(ty) => crate::lower::type_ref::lower_type_ref(ctx.ctx, *ty),
            HirGenericArg::Wildcard { .. } | HirGenericArg::EffectRow(_) => None,
        })
        .collect::<Vec<_>>();
    if method == "run" {
        collect_expr(ctx, receiver, None)?;
    }
    if let etas_hir::HirExpr::Path(path) = &ctx.ctx.hir.exprs[receiver]
        && let etas_hir::ResolveResult::Resolved(symbol) = path.resolution
    {
        let candidates = super::std_member::std_type_member_candidates_for_symbol(
            ctx,
            symbol,
            method,
            checked_generic_args.is_empty(),
        )
        .into_iter()
        .filter(|candidate| {
            matches!(
                candidate.operation,
                crate::CallableCandidateOperation::StdIntrinsic {
                    has_receiver: false,
                    ..
                }
            )
        })
        .collect::<Vec<_>>();
        if !candidates.is_empty() {
            return super::std_type_member::collect(
                ctx,
                super::std_type_member::Call {
                    expr,
                    callee: None,
                    method,
                    candidates,
                    generic_args: &checked_generic_args,
                    args,
                    span,
                    expected,
                },
            );
        }
    }
    let (std_member, generic_params) =
        if let Some(signature) = std_member_callable_signature(ctx, receiver, method) {
            let candidate = super::expr::callable_candidate_from_fact(
                ctx,
                crate::SymbolTypeFact::Flow { signature },
                type_generic_args.is_empty(),
            );
            match candidate {
                Some(candidate) => (Some(candidate.ty), candidate.generic_params),
                None => (None, Vec::new()),
            }
        } else {
            let member = if type_generic_args.is_empty() {
                std_member_value_type(ctx, receiver, method)
            } else {
                raw_std_member_value_type(ctx, receiver, method)
            };
            (member, Vec::new())
        };
    if let Some(callee_ty) = std_member {
        let expected_inputs =
            super::call::callable_input_types(ctx, callee_ty, &type_generic_args, span);
        let arg_tys = args
            .iter()
            .enumerate()
            .map(|(index, arg)| match arg {
                HirArg::Positional(expr) => collect_expr(
                    ctx,
                    *expr,
                    expected_inputs
                        .as_ref()
                        .and_then(|inputs| inputs.get(index).copied()),
                ),
                HirArg::Named { value, .. } => collect_expr(
                    ctx,
                    *value,
                    expected_inputs
                        .as_ref()
                        .and_then(|inputs| inputs.get(index).copied()),
                ),
            })
            .collect::<Result<Vec<_>, E>>()?;
        let output = expected.unwrap_or_else(|| {
            specialized_callable_output_for_args(ctx, callee_ty, &type_generic_args, &arg_tys, span)
                .or_else(|| callable_output_for_expression(ctx, callee_ty))
                .unwrap_or_else(|| ctx.fresh_type_var())
        });
        ctx.validate(crate::ValidationRequest::CallableArity {
            callee_ty,
            arg_count: arg_tys.len(),
            span,
        });
        ctx.emit(TypeConstraint::Callable {
            call: Some(expr),
            callee: callee_ty,
            generic_params,
            generic_args: checked_generic_args,
            arg_exprs: args
                .iter()
                .map(|arg| {
                    Some(match arg {
                        HirArg::Positional(expr) => *expr,
                        HirArg::Named { value, .. } => *value,
                    })
                })
                .collect(),
            args: arg_tys,
            output,
            origin: ConstraintOrigin { span },
        });
        return Ok(output);
    }

    let mut candidates = if type_generic_args.is_empty() {
        std_method_candidates(ctx, method)
    } else {
        raw_std_method_candidates(ctx, method)
    };
    candidates.extend(super::agent_method::deferred_candidates(
        ctx,
        method,
        &checked_generic_args,
        span,
    ));
    if !candidates.is_empty() || super::agent_method::has_method_name(ctx, method) {
        let expected_inputs = common_candidate_inputs(ctx, &candidates);
        let receiver_expected = expected_inputs
            .as_ref()
            .and_then(|inputs| inputs.first().copied());
        let receiver_ty = collect_expr(ctx, receiver, receiver_expected)?;
        let mut arg_tys = Vec::with_capacity(args.len() + 1);
        arg_tys.push(receiver_ty);
        let arguments = args
            .iter()
            .enumerate()
            .map(|(index, arg)| {
                let expected = expected_inputs
                    .as_ref()
                    .and_then(|inputs| inputs.get(index + 1).copied());
                match arg {
                    HirArg::Positional(expr) => collect_expr(ctx, *expr, expected),
                    HirArg::Named { value, .. } => collect_expr(ctx, *value, expected),
                }
            })
            .collect::<Result<Vec<_>, E>>()?;
        arg_tys.extend(arguments);
        let output = expected.unwrap_or_else(|| {
            specialized_method_output_for_args(ctx, &candidates, &type_generic_args, &arg_tys, span)
                .unwrap_or_else(|| ctx.fresh_type_var())
        });
        ctx.emit(TypeConstraint::MethodCall {
            expr,
            method: method.to_owned(),
            access_module: ctx.ctx.hir.item_module(ctx.state.item),
            candidates,
            generic_args: checked_generic_args,
            arg_exprs: std::iter::once(Some(receiver))
                .chain(args.iter().map(|arg| {
                    Some(match arg {
                        HirArg::Positional(expr) => *expr,
                        HirArg::Named { value, .. } => *value,
                    })
                }))
                .collect(),
            args: arg_tys,
            output,
            origin: ConstraintOrigin { span },
        });
        return Ok(output);
    }

    let receiver_ty = collect_expr(ctx, receiver, None)?;
    let output = expected.unwrap_or_else(|| ctx.fresh_type_var());
    ctx.emit(TypeConstraint::FieldAccess {
        base: receiver_ty,
        field: method.to_owned(),
        output,
        origin: ConstraintOrigin { span },
    });
    Ok(output)
}

fn common_candidate_inputs<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    candidates: &[crate::CallableCandidate],
) -> Option<Vec<TypeId>> {
    let mut expected: Option<Vec<TypeId>> = None;
    for candidate in candidates {
        let Some(crate::Type::Function(flow)) = ctx.ctx.interner.store().get(candidate.ty) else {
            return None;
        };
        if let Some(existing) = &expected {
            if existing != &flow.input {
                return None;
            }
        } else {
            expected = Some(flow.input.clone());
        }
    }
    let params = expected?;
    let candidate = candidates.first()?;
    let needs_instantiation = candidate.generic_params.iter().any(|param| {
        param.kind == crate::CallableGenericParamKind::Type
            && matches!(
                ctx.ctx.interner.store().get(param.subject),
                Some(crate::Type::Named(_))
            )
    });
    if !needs_instantiation {
        return Some(params);
    }
    // Context hints belong to this call site, not to the method declaration.
    // Keep the original candidates for solving explicit arguments and bounds.
    let output = ctx.primitive(crate::PrimitiveType::Unit);
    super::callable_instantiation::instantiate_signature(
        ctx,
        crate::CallableSignature {
            generic_params: candidate.generic_params.clone(),
            params,
            output,
            effects: None,
            requested_actions: None,
        },
    )
    .map(|signature| signature.params)
}
