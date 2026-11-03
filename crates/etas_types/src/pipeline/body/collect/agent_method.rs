use crate::pipeline::context::BodyCollectContext;
use crate::{CallableGenericArg, ConstraintOrigin, SymbolTypeFact, TypeConstraint, TypeId};

pub(super) fn has_method_name<E>(ctx: &BodyCollectContext<'_, '_, E>, name: &str) -> bool {
    let facts = ctx
        .ctx
        .signature_base
        .map_or(&ctx.ctx.signature_facts, |base| &base.facts);
    facts
        .agent_methods
        .values()
        .any(|method| method.name == name)
}

pub(super) fn deferred_candidates<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    name: &str,
    arguments: &[CallableGenericArg],
    span: etas_core::Span,
) -> Vec<crate::CallableCandidate> {
    let facts = ctx
        .ctx
        .signature_base
        .map_or(&ctx.ctx.signature_facts, |base| &base.facts);
    let mut methods = facts
        .agent_methods
        .values()
        .filter(|method| method.name == name)
        .cloned()
        .collect::<Vec<_>>();
    methods.sort_by_key(|method| method.symbol);
    let mut candidates = vec![];
    for method in methods {
        let Some(mut candidate) = super::expr::callable_candidate_from_fact(
            ctx,
            SymbolTypeFact::Flow {
                signature: method.callable,
            },
            true,
        ) else {
            continue;
        };
        let _ = super::callable_instantiation::signature_hints(
            ctx,
            candidate.ty,
            &candidate.signature.generic_params,
            arguments,
            span,
        );
        let Some(crate::Type::Function(mut signature)) =
            ctx.ctx.interner.store().get(candidate.ty).cloned()
        else {
            continue;
        };
        candidate.operation = crate::CallableCandidateOperation::AgentMethod {
            symbol: method.symbol,
            callable: candidate.ty,
        };
        signature.input.insert(0, method.component);
        candidate.ty = ctx.ctx.interner.intern(crate::Type::Function(signature));
        candidates.push(candidate);
    }
    candidates
}

pub fn validate_method_access<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    symbol: etas_hir::SymbolId,
    span: etas_core::Span,
) {
    let facts = ctx
        .ctx
        .signature_base
        .map_or(&ctx.ctx.signature_facts, |base| &base.facts);
    if let Some(method) = facts.agent_methods.get(&symbol)
        && method.private_to.is_some()
        && method.private_to != ctx.ctx.hir.item_module(ctx.state.item)
    {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span,
            message: "agent method is private to its defining module".into(),
        });
    }
}

pub fn validate_default_call_access<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    callee: HirExprId,
    span: etas_core::Span,
) {
    let Some(component) = receiver_component(ctx, callee) else {
        return;
    };
    let facts = ctx
        .ctx
        .signature_base
        .map_or(&ctx.ctx.signature_facts, |base| &base.facts);
    let method = facts
        .agent_signature(component)
        .and_then(|signature| signature.default_entry.as_ref())
        .and_then(|entry| entry.method);
    if let Some(method) = method {
        validate_method_access(ctx, method, span);
    }
}
use etas_hir::{HirArg, HirExpr, HirExprId, HirGenericArg, ResolveResult};

pub fn receiver_component<E>(
    ctx: &BodyCollectContext<'_, '_, E>,
    receiver: HirExprId,
) -> Option<TypeId> {
    if let Some(ty) = ctx.state.provisional.expr_types.get(&receiver) {
        return ctx
            .ctx
            .agent_signature(*ty)
            .map(|signature| signature.component);
    }
    let HirExpr::Path(path) = ctx.ctx.hir.exprs.get(receiver)? else {
        return None;
    };
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return None;
    };
    symbol_component(ctx, symbol)
}

fn symbol_component<E>(
    ctx: &BodyCollectContext<'_, '_, E>,
    symbol: etas_hir::SymbolId,
) -> Option<TypeId> {
    let fact = ctx
        .state
        .provisional
        .symbol_types
        .get(&symbol)
        .or_else(|| ctx.ctx.symbol_fact(symbol))?;
    match fact {
        SymbolTypeFact::Agent { signature } => Some(signature.component),
        SymbolTypeFact::Param { ty }
        | SymbolTypeFact::Local { ty, .. }
        | SymbolTypeFact::Value { ty }
        | SymbolTypeFact::TopLevelLet { ty, .. } => ctx
            .ctx
            .agent_signature(*ty)
            .map(|signature| signature.component),
        _ => None,
    }
}

pub fn collect_agent_method_call<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    expr: HirExprId,
    receiver: HirExprId,
    name: &str,
    generic_args: &[HirGenericArg],
    args: &[HirArg],
    expected: Option<TypeId>,
) -> Result<Option<TypeId>, E> {
    ctx.checkpoint()?;
    let Some(component) = receiver_component(ctx, receiver) else {
        return Ok(None);
    };
    super::expr::collect_expr(ctx, receiver, None)?;
    collect_selected_call(ctx, expr, component, name, generic_args, args, expected)
}

pub fn collect_agent_path_call<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    expr: HirExprId,
    callee: HirExprId,
    generic_args: &[HirGenericArg],
    args: &[HirArg],
    expected: Option<TypeId>,
) -> Result<Option<TypeId>, E> {
    ctx.checkpoint()?;
    let Some(HirExpr::Path(path)) = ctx.ctx.hir.exprs.get(callee) else {
        return Ok(None);
    };
    let ResolveResult::PartiallyResolved(partial) = &path.resolution else {
        return Ok(None);
    };
    let [name] = partial.remaining.as_slice() else {
        return Ok(None);
    };
    let name = name.clone();
    let Some(component) = partial
        .resolved_prefix
        .and_then(|symbol| symbol_component(ctx, symbol))
    else {
        return Ok(None);
    };
    collect_selected_call(ctx, expr, component, &name, generic_args, args, expected)
}

fn collect_selected_call<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    expr: HirExprId,
    component: TypeId,
    name: &str,
    generic_args: &[HirGenericArg],
    args: &[HirArg],
    expected: Option<TypeId>,
) -> Result<Option<TypeId>, E> {
    ctx.checkpoint()?;
    let facts = ctx
        .ctx
        .signature_base
        .map_or(&ctx.ctx.signature_facts, |base| &base.facts);
    let mut methods = facts
        .agent_methods
        .values()
        .filter(|method| method.component == component && method.name == name)
        .collect::<Vec<_>>();
    if methods.is_empty() {
        return Ok(None);
    }
    let inherent = methods
        .iter()
        .filter(|method| method.inherent)
        .copied()
        .collect::<Vec<_>>();
    if !inherent.is_empty() {
        methods = inherent;
    }
    let span = ctx.ctx.hir.exprs[expr].span(&ctx.ctx.hir.blocks);
    let [method] = methods.as_slice() else {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: etas_core::TypeDiagnosticCode::TypeMismatch,
            span,
            message: "agent method is ambiguous; select the defining spec explicitly".into(),
        });
        return Ok(Some(ctx.primitive(crate::PrimitiveType::Never)));
    };
    let method = (*method).clone();
    validate_method_access(ctx, method.symbol, span);
    let Some(candidate) = super::expr::callable_candidate_from_fact(
        ctx,
        SymbolTypeFact::Flow {
            signature: method.callable,
        },
        true,
    ) else {
        return Ok(Some(ctx.primitive(crate::PrimitiveType::Never)));
    };
    let mut checked_args = Vec::new();
    for arg in generic_args {
        ctx.checkpoint()?;
        match arg {
            HirGenericArg::Type(ty) => {
                if let Some(ty) = crate::lower::type_ref::lower_type_ref(ctx.ctx, *ty) {
                    checked_args.push(CallableGenericArg::Type(ty));
                }
            }
            HirGenericArg::EffectRow(row) => checked_args.push(CallableGenericArg::EffectRow(
                crate::lower::effect_row::lower_effect_row(ctx.ctx, row),
            )),
            HirGenericArg::Wildcard { span } => {
                ctx.validate(crate::ValidationRequest::Diagnostic {
                    code: etas_core::TypeDiagnosticCode::TypeMismatch,
                    span: *span,
                    message: "agent method generic arguments cannot be wildcard selectors".into(),
                })
            }
        }
    }
    let hints = super::callable_instantiation::signature_hints(
        ctx,
        candidate.ty,
        &candidate.signature.generic_params,
        &checked_args,
        span,
    );
    let actual = args
        .iter()
        .enumerate()
        .map(|(index, arg)| {
            let value = match arg {
                HirArg::Positional(value) | HirArg::Named { value, .. } => *value,
            };
            super::expr::collect_expr(
                ctx,
                value,
                hints
                    .as_ref()
                    .and_then(|signature| signature.input.get(index))
                    .copied(),
            )
        })
        .collect::<Result<Vec<_>, E>>()?;
    let output = expected
        .or_else(|| hints.as_ref().map(|signature| signature.output))
        .unwrap_or_else(|| ctx.fresh_type_var());
    ctx.emit(TypeConstraint::Callable {
        call: Some(expr),
        callee: candidate.ty,
        generic_params: candidate.signature.generic_params,
        generic_args: checked_args,
        arg_exprs: args
            .iter()
            .map(|arg| {
                Some(match arg {
                    HirArg::Positional(value) | HirArg::Named { value, .. } => *value,
                })
            })
            .collect(),
        args: actual,
        output,
        origin: ConstraintOrigin { span },
    });
    ctx.state
        .provisional
        .agent_method_calls
        .insert(expr, method.symbol);
    Ok(Some(output))
}
