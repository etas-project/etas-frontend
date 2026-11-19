use crate::{
    CallableCandidate, CallableGenericArg, ConstraintOrigin, Type, TypeConstraint, TypeId,
    pipeline::context::BodyCollectContext,
};
use etas_hir::{HirArg, HirExprId};
use std::collections::HashMap;

pub(super) struct Call<'a> {
    pub expr: HirExprId,
    pub callee: Option<HirExprId>,
    pub candidates: Vec<CallableCandidate>,
    pub generic_args: &'a [CallableGenericArg],
    pub args: &'a [HirArg],
    pub span: etas_core::Span,
    pub expected: Option<TypeId>,
}

pub(super) fn collect<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    call: Call<'_>,
) -> Result<TypeId, E> {
    let Call {
        expr,
        callee,
        candidates,
        generic_args,
        args,
        span,
        expected,
    } = call;
    for candidate in &candidates {
        super::agent_context::constrain(ctx, candidate, span);
    }
    let arity_matches = candidates.iter().filter(|candidate| {
        matches!(ctx.ctx.interner.store().get(candidate.ty), Some(Type::Function(flow)) if flow.input.len() == args.len())
    }).collect::<Vec<_>>();
    let hint = if let [candidate] = arity_matches.as_slice() {
        if let Some(callee) = callee {
            ctx.record_expr_type(callee, candidate.ty);
        }
        let substitutions = candidate
            .signature
            .generic_params
            .iter()
            .zip(generic_args)
            .filter_map(|(param, arg)| match arg {
                CallableGenericArg::Type(ty) => Some((param.name.clone(), *ty)),
                CallableGenericArg::EffectRow(_) => None,
            })
            .collect::<HashMap<_, _>>();
        let hinted =
            super::call::substitute_named_or_report(ctx, candidate.ty, &substitutions, span);
        match ctx.ctx.interner.store().get(hinted) {
            Some(Type::Function(flow)) => Some(flow.clone()),
            _ => None,
        }
    } else {
        None
    };
    let arg_tys =
        super::call::collect_call_args(ctx, args, hint.as_ref().map(|flow| flow.input.as_slice()))?;
    let inferred = hint
        .filter(|_| !generic_args.is_empty())
        .map(|flow| flow.output);
    let output = super::call::select_call_output(ctx, inferred, expected, span);
    let callee = ctx.fresh_type_var();
    let probes = candidates.iter().map(|_| ctx.fresh_type_var()).collect();
    ctx.emit(TypeConstraint::OverloadedCall {
        expr,
        callee,
        probes,
        candidates,
        generic_args: generic_args.to_vec(),
        arg_exprs: super::call::call_arg_exprs(args),
        args: arg_tys,
        output,
        origin: ConstraintOrigin { span },
    });
    Ok(output)
}
