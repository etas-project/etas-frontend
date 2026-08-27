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

pub fn collect_method_call(
    ctx: &mut BodyCollectContext<'_, '_>,
    receiver: etas_hir::HirExprId,
    method: &str,
    generic_args: &[HirGenericArg],
    args: &[HirArg],
    span: etas_core::Span,
    expected: Option<TypeId>,
) -> TypeId {
    if generic_args.iter().any(|arg| {
        matches!(
            arg,
            HirGenericArg::EffectRow(_) | HirGenericArg::Wildcard { .. }
        )
    }) {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span,
            message: "only type generic arguments are valid for method calls".to_owned(),
        });
    }
    let type_generic_args = generic_args
        .iter()
        .filter_map(|arg| match arg {
            HirGenericArg::Type(ty) => crate::lower::type_ref::lower_type_ref(ctx.ctx, *ty),
            HirGenericArg::Wildcard { .. } | HirGenericArg::EffectRow(_) => None,
        })
        .collect::<Vec<_>>();
    if method == "run" {
        collect_expr(ctx, receiver, None);
    }
    let generic_params = std_member_callable_signature(ctx, receiver, method)
        .map(|signature| signature.generic_params)
        .unwrap_or_default();
    let std_member = if type_generic_args.is_empty() && generic_params.is_empty() {
        std_member_value_type(ctx, receiver, method)
    } else {
        raw_std_member_value_type(ctx, receiver, method)
    };
    if let Some(callee_ty) = std_member {
        let arg_tys = args
            .iter()
            .map(|arg| match arg {
                HirArg::Positional(expr) => collect_expr(ctx, *expr, None),
                HirArg::Named { value, .. } => collect_expr(ctx, *value, None),
            })
            .collect::<Vec<_>>();
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
            call: None,
            callee: callee_ty,
            generic_params,
            generic_args: type_generic_args,
            args: arg_tys,
            output,
            origin: ConstraintOrigin { span },
        });
        return output;
    }

    let candidates = if type_generic_args.is_empty() {
        std_method_candidates(ctx, method)
    } else {
        raw_std_method_candidates(ctx, method)
    };
    if !candidates.is_empty() {
        let expected_inputs = common_candidate_inputs(ctx, &candidates);
        let receiver_expected = expected_inputs
            .as_ref()
            .and_then(|inputs| inputs.first().copied());
        let receiver_ty = collect_expr(ctx, receiver, receiver_expected);
        let mut arg_tys = Vec::with_capacity(args.len() + 1);
        arg_tys.push(receiver_ty);
        arg_tys.extend(args.iter().enumerate().map(|(index, arg)| {
            let expected = expected_inputs
                .as_ref()
                .and_then(|inputs| inputs.get(index + 1).copied());
            match arg {
                HirArg::Positional(expr) => collect_expr(ctx, *expr, expected),
                HirArg::Named { value, .. } => collect_expr(ctx, *value, expected),
            }
        }));
        let output = expected.unwrap_or_else(|| {
            specialized_method_output_for_args(ctx, &candidates, &type_generic_args, &arg_tys, span)
                .unwrap_or_else(|| ctx.fresh_type_var())
        });
        ctx.emit(TypeConstraint::MethodCall {
            method: method.to_owned(),
            candidates,
            generic_args: type_generic_args,
            args: arg_tys,
            output,
            origin: ConstraintOrigin { span },
        });
        return output;
    }

    let receiver_ty = collect_expr(ctx, receiver, None);
    let output = expected.unwrap_or_else(|| ctx.fresh_type_var());
    ctx.emit(TypeConstraint::FieldAccess {
        base: receiver_ty,
        field: method.to_owned(),
        output,
        origin: ConstraintOrigin { span },
    });
    output
}

fn common_candidate_inputs(
    ctx: &BodyCollectContext<'_, '_>,
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
    expected
}
