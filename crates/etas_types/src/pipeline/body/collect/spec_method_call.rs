use etas_core::{TypeDiagnosticCode, TypeDiagnosticCode::TypeMismatch};
use etas_hir::{HirArg, ResolveResult, ResolvedPath, SymbolId};

use crate::{
    ConstraintOrigin, FlowType, SpecObligation, SymbolTypeFact, Type, TypeConstraint, TypeId,
    lower::type_ref::lower_type_ref,
    pipeline::{body::collect::expr::collect_expr, context::BodyCollectContext},
};

pub struct SpecMethodCallInput<'a> {
    pub receiver: etas_hir::HirExprId,
    pub spec_path: &'a ResolvedPath,
    pub spec_args: &'a [etas_hir::HirTypeId],
    pub method: &'a str,
    pub args: &'a [HirArg],
    pub span: etas_core::Span,
    pub expected: Option<TypeId>,
}

pub fn collect_spec_method_call<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    input: SpecMethodCallInput<'_>,
) -> Result<TypeId, E> {
    ctx.checkpoint()?;
    let SpecMethodCallInput {
        receiver,
        spec_path,
        spec_args,
        method,
        args,
        span,
        expected,
    } = input;
    let receiver_ty = collect_expr(ctx, receiver, None)?;
    let explicit_arg_tys = args
        .iter()
        .map(|arg| match arg {
            HirArg::Positional(expr) => collect_expr(ctx, *expr, None),
            HirArg::Named { value, .. } => collect_expr(ctx, *value, None),
        })
        .collect::<Result<Vec<_>, E>>()?;

    let Some(spec_symbol) = resolved_spec_symbol(ctx, spec_path) else {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: TypeDiagnosticCode::UnknownType,
            span,
            message: "spec method selection must name a resolved spec".to_owned(),
        });
        return Ok(expected.unwrap_or_else(|| ctx.primitive(crate::PrimitiveType::Never)));
    };

    let Some(signature) = ctx.ctx.spec_signature(spec_symbol).cloned() else {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: TypeDiagnosticCode::IncompleteTypeFacts,
            span,
            message: "spec method selection is missing checked spec signature facts".to_owned(),
        });
        return Ok(expected.unwrap_or_else(|| ctx.primitive(crate::PrimitiveType::Never)));
    };

    let Some(spec_arg_tys) = lower_spec_args(ctx, spec_args, span) else {
        return Ok(expected.unwrap_or_else(|| ctx.primitive(crate::PrimitiveType::Never)));
    };
    if spec_arg_tys.len() != signature.params.len() {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: TypeDiagnosticCode::ArityMismatch,
            span,
            message: format!(
                "spec `{}` expects {} type argument(s), got {}",
                signature.name,
                signature.params.len(),
                spec_arg_tys.len()
            ),
        });
        return Ok(expected.unwrap_or_else(|| ctx.primitive(crate::PrimitiveType::Never)));
    }

    let Some(method_fact) = signature
        .methods
        .iter()
        .find(|candidate| candidate.name == method)
    else {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: TypeMismatch,
            span,
            message: format!(
                "spec `{}` does not declare method `{method}`",
                signature.name
            ),
        });
        return Ok(expected.unwrap_or_else(|| ctx.primitive(crate::PrimitiveType::Never)));
    };

    let Some(callee_ty) = method_callable_type(ctx, method_fact) else {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: TypeDiagnosticCode::IncompleteTypeFacts,
            span,
            message: format!("spec method `{method}` is missing callable signature facts"),
        });
        return Ok(expected.unwrap_or_else(|| ctx.primitive(crate::PrimitiveType::Never)));
    };

    ctx.state.spec_obligations.push(SpecObligation {
        ty: receiver_ty,
        spec: crate::CheckedSpecRef::Source(spec_symbol),
        args: spec_arg_tys.clone(),
        span,
    });

    let mut call_args = Vec::with_capacity(explicit_arg_tys.len() + 1);
    call_args.push(receiver_ty);
    call_args.extend(explicit_arg_tys);
    let output = expected.unwrap_or_else(|| ctx.fresh_type_var());
    ctx.emit(TypeConstraint::Callable {
        call: None,
        callee: callee_ty,
        generic_params: signature
            .param_names
            .into_iter()
            .zip(spec_arg_tys.iter().copied())
            .map(|(name, subject)| crate::CallableGenericParam {
                kind: crate::CallableGenericParamKind::Type,
                name,
                subject,
                bounds: Vec::new(),
            })
            .collect(),
        generic_args: spec_arg_tys
            .into_iter()
            .map(crate::CallableGenericArg::Type)
            .collect(),
        arg_exprs: std::iter::once(Some(receiver))
            .chain(args.iter().map(|arg| {
                Some(match arg {
                    HirArg::Positional(expr) => *expr,
                    HirArg::Named { value, .. } => *value,
                })
            }))
            .collect(),
        args: call_args,
        output,
        origin: ConstraintOrigin { span },
    });
    Ok(output)
}

fn resolved_spec_symbol<E>(
    ctx: &BodyCollectContext<'_, '_, E>,
    path: &ResolvedPath,
) -> Option<SymbolId> {
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return None;
    };
    let symbol = ctx
        .ctx
        .symbols
        .canonical_symbol(ctx.ctx.hir, symbol)
        .unwrap_or(symbol);
    ctx.ctx.spec_signature(symbol).is_some().then_some(symbol)
}

fn lower_spec_args<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    spec_args: &[etas_hir::HirTypeId],
    span: etas_core::Span,
) -> Option<Vec<TypeId>> {
    let mut lowered = Vec::with_capacity(spec_args.len());
    for arg in spec_args {
        let Some(ty) = lower_type_ref(ctx.ctx, *arg) else {
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: TypeDiagnosticCode::UnknownType,
                span,
                message: "spec method type argument could not be resolved".to_owned(),
            });
            return None;
        };
        lowered.push(ty);
    }
    Some(lowered)
}

fn method_callable_type<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    method: &crate::SpecMethodFact,
) -> Option<TypeId> {
    let signature = if let Some(signature) = &method.signature {
        signature.clone()
    } else {
        let symbol = method.source_symbol()?;
        let fact = ctx.ctx.symbol_fact(symbol)?.clone();
        let SymbolTypeFact::Flow { signature } = fact else {
            return None;
        };
        signature
    };
    Some(ctx.ctx.interner.intern(Type::Function(FlowType {
        input: signature.params,
        output: signature.output,
        effects: signature.effects,
    })))
}
