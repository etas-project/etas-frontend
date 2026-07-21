use etas_hir::{HirArg, HirGenericArg, ResolveResult, ResolvedActionRef};

use crate::{
    AssignabilityReason, ConstraintOrigin, EffectActionSignature, PrimitiveType, SymbolTypeFact,
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

pub fn collect_perform(
    ctx: &mut BodyCollectContext<'_, '_>,
    action: &ResolvedActionRef,
    generic_args: &[HirGenericArg],
    args: &[HirArg],
    span: etas_core::Span,
    expected: Option<TypeId>,
) -> TypeId {
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
            collect_arg(ctx, arg, None);
        }
        return expected.unwrap_or_else(|| ctx.primitive(PrimitiveType::Never));
    };
    let signature = specialize_action_signature(ctx, signature, &action.effect.args);
    validate_action_selector_arity_and_kinds(
        ctx,
        &signature,
        generic_args,
        span,
        "performed action",
    );
    let signature = specialize_action_signature_from_generic_args(ctx, signature, generic_args);

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

    for (index, arg) in args.iter().enumerate() {
        let expected_arg = signature.params.get(index).copied();
        let actual = collect_arg(ctx, arg, expected_arg);
        if let Some(expected_arg) = expected_arg {
            ctx.emit(TypeConstraint::Assignable {
                from: actual,
                to: expected_arg,
                origin: ConstraintOrigin {
                    span: arg_span(arg, span),
                },
                reason: AssignabilityReason::Argument,
            });
        }
    }

    if let Some(expected) = expected {
        ctx.emit(TypeConstraint::Assignable {
            from: signature.output,
            to: expected,
            origin: ConstraintOrigin { span },
            reason: AssignabilityReason::Other,
        });
    }
    signature.output
}

fn action_signature(
    ctx: &BodyCollectContext<'_, '_>,
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
        .or_else(|| {
            ctx.ctx
                .symbols
                .symbol_fact(ctx.ctx.hir, &ctx.ctx.signature_facts, symbol)
        })
        .and_then(|fact| match fact {
            SymbolTypeFact::EffectAction { signature } => Some(signature.clone()),
            _ => None,
        })
}

fn collect_arg(
    ctx: &mut BodyCollectContext<'_, '_>,
    arg: &HirArg,
    expected: Option<TypeId>,
) -> TypeId {
    match arg {
        HirArg::Positional(expr) => collect_expr(ctx, *expr, expected),
        HirArg::Named { value, .. } => collect_expr(ctx, *value, expected),
    }
}

fn arg_span(arg: &HirArg, fallback: etas_core::Span) -> etas_core::Span {
    match arg {
        HirArg::Positional(_) => fallback,
        HirArg::Named { span, .. } => *span,
    }
}
