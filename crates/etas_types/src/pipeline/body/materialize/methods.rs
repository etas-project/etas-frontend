use std::collections::{BTreeSet, HashMap};

use etas_hir::{
    HirExpr, HirExprId, HirItem, HirItemId, HirPat, HirProgram, HirStmt, ResolveResult, SymbolId,
};

use crate::{
    FlowType, Type,
    facts::methods::{CheckedMethodCallFact, MethodCallKind},
    method::MethodTarget,
    pipeline::context::TypePipelineContext,
};

use super::BodyPipelineState;

pub(super) fn materialize(
    ctx: &mut TypePipelineContext<'_>,
    state: &mut BodyPipelineState,
) -> Result<(), String> {
    let owner = state.item;
    let receiver = match ctx.hir.items.get(owner) {
        Some(etas_hir::HirItem::Flow(flow))
            if flow.receiver_mode == etas_hir::HirReceiverMode::Updating
                && ctx
                    .signature_facts
                    .type_impl_methods
                    .values()
                    .chain(
                        ctx.signature_base
                            .into_iter()
                            .flat_map(|base| base.facts.type_impl_methods.values()),
                    )
                    .any(|method| {
                        method.body == owner
                            && ctx
                                .agent_signature(method.self_type)
                                .is_some_and(|agent| agent.state.is_some())
                    }) =>
        {
            flow.params.first().copied()
        }
        _ => None,
    };
    let specs = ctx.spec_facts();
    let view = receiver.map(|_| {
        ctx.hir_view
            .get_or_insert_with(|| etas_hir::HirTreeView::new(ctx.hir))
            .clone()
    });
    let bound_values = state
        .solver_report
        .bound_method_values
        .keys()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    let mut methods = state
        .solver_report
        .checked_method_calls
        .iter_mut()
        .chain(
            state
                .solver_report
                .bound_method_values
                .iter_mut()
                .map(|(expr, value)| (expr, &mut value.method)),
        )
        .collect::<Vec<_>>();
    methods.sort_unstable_by_key(|(expr, _)| **expr);
    for (expr, fact) in methods {
        if fact.receiver.value_slot().is_some()
            && crate::active_agent::supports(ctx.interner.store(), &specs, fact.receiver_type)
        {
            fact.kind = MethodCallKind::AgentRequest;
            let identity = match &fact.target {
                MethodTarget::Source { symbol, .. } | MethodTarget::Agent { symbol, .. } => {
                    Some((*symbol).into())
                }
                MethodTarget::External { identity } => {
                    Some(crate::AgentContextRef::Portable(Box::new(identity.clone())))
                }
                // A spec member has no implementation context until its
                // receiver evidence resolves a concrete method.
                _ => None,
            };
            fact.request_context =
                identity.map(|identity| ctx.interner.intern(Type::AgentContext(identity)));
            let direct = request_receiver(ctx.hir, *expr);
            let in_body = view
                .as_ref()
                .is_some_and(|view| MethodCallKind::same_callable_body(view, *expr, owner));
            if receiver.is_some() && direct == receiver && in_body && !bound_values.contains(expr) {
                fact.kind = MethodCallKind::AgentTurnHelper;
            }
        }
        if fact
            .instantiation
            .deferred_effect_row_obligations
            .is_empty()
        {
            let declaration = ctx
                .methods()
                .declaration(&fact.target, &fact.evidence)
                .ok_or("checked method has no unique declaration")?
                .clone();
            fact.signature =
                crate::method::instantiate::signature(&mut ctx.interner, &declaration, fact)?;
            let output = if bound_values.contains(expr) {
                let (crate::method::MethodReceiver::Value { slot }
                | crate::method::MethodReceiver::Updating { slot }) = fact.receiver
                else {
                    unreachable!()
                };
                let mut input = fact.signature.params.clone();
                input.remove(slot as usize);
                ctx.interner.intern(Type::Function(FlowType {
                    input,
                    output: fact.signature.output,
                    effects: fact.signature.effects.clone(),
                }))
            } else {
                fact.signature.output
            };
            state
                .solver_report
                .inferred_expr_types
                .insert(*expr, output);
        }
        match &fact.target {
            MethodTarget::Standard {
                intrinsic: Some(intrinsic),
                qualified_path,
                overload,
                ..
            } => {
                let mut runtime_signature = fact.signature.clone();
                if let crate::method::MethodReceiver::Value { slot } = fact.receiver {
                    let receiver = runtime_signature
                        .params
                        .get_mut(slot as usize)
                        .ok_or("checked standard method receiver slot is out of bounds")?;
                    *receiver = fact.receiver_type;
                }
                let callable_type = ctx.interner.intern(Type::Function(FlowType {
                    input: runtime_signature.params.clone(),
                    output: runtime_signature.output,
                    effects: runtime_signature.effects.clone(),
                }));
                state.solver_report.std_intrinsic_calls.insert(
                    *expr,
                    crate::facts::CheckedStdCallFact {
                        qualified_path: qualified_path.clone(),
                        intrinsic: *intrinsic,
                        overload: *overload,
                        callable_type,
                        signature: runtime_signature,
                    },
                );
            }
            MethodTarget::Agent { symbol, .. } => {
                state
                    .solver_report
                    .agent_method_calls
                    .insert(*expr, *symbol);
            }
            _ => {}
        }
    }
    if let Some(receiver) = receiver
        && let Some(call) = unconditional_self_request(
            ctx.hir,
            owner,
            receiver,
            &state.solver_report.checked_method_calls,
        )
    {
        ctx.diagnostics.push(etas_core::Diagnostic::type_check(
            etas_core::TypeDiagnosticCode::UnavoidableAgentWait,
            ctx.hir.exprs[call].span(&ctx.hir.blocks),
            "this Agent request must wait for the update turn already held by the current method",
        ));
    }
    Ok(())
}

fn resolved_symbol(hir: &HirProgram, expr: HirExprId) -> Option<SymbolId> {
    let HirExpr::Path(path) = hir.exprs.get(expr)? else {
        return None;
    };
    match path.resolution {
        ResolveResult::Resolved(symbol) => Some(symbol),
        _ => None,
    }
}

fn request_receiver(hir: &HirProgram, expr: HirExprId) -> Option<SymbolId> {
    match hir.exprs.get(expr)? {
        HirExpr::MethodCall { receiver, .. } => resolved_symbol(hir, *receiver),
        HirExpr::Call { callee, .. } => match hir.exprs.get(*callee)? {
            HirExpr::Path(path) => match &path.resolution {
                ResolveResult::PartiallyResolved(path) if path.remaining.len() == 1 => {
                    path.resolved_prefix
                }
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

fn unconditional_self_request(
    hir: &HirProgram,
    owner: HirItemId,
    receiver: SymbolId,
    calls: &HashMap<HirExprId, CheckedMethodCallFact>,
) -> Option<HirExprId> {
    let HirItem::Flow(flow) = hir.items.get(owner)? else {
        return None;
    };
    let body = hir.blocks.get(flow.body.as_block()?)?;
    let prove = |expr: HirExprId, aliases: &BTreeSet<SymbolId>| {
        let selected = calls.get(&expr)?;
        if selected.kind != MethodCallKind::AgentRequest
            || !selected.receiver.is_updating()
            || !selected.defaults.is_empty()
            || !aliases.contains(&request_receiver(hir, expr)?)
        {
            return None;
        }
        let args = match hir.exprs.get(expr)? {
            HirExpr::Call { args, .. } | HirExpr::MethodCall { args, .. } => args,
            _ => return None,
        };
        args.is_empty().then_some(expr)
    };
    let mut aliases = BTreeSet::from([receiver]);
    for stmt in &body.stmts {
        match hir.stmts.get(*stmt)? {
            HirStmt::Let { pat, value, .. } | HirStmt::Var { pat, value, .. } => {
                if let Some(call) = prove(*value, &aliases) {
                    return Some(call);
                }
                let binding = match hir.pats.get(*pat)? {
                    HirPat::Binding { symbol, .. } => Some(*symbol),
                    HirPat::Wildcard { .. } => None,
                    _ => return None,
                };
                if let Some(source) = resolved_symbol(hir, *value) {
                    if aliases.contains(&source)
                        && let Some(binding) = binding
                    {
                        aliases.insert(binding);
                    }
                } else if !matches!(hir.exprs.get(*value), Some(HirExpr::Literal(_))) {
                    // An earlier computation may decide whether this call is
                    // reached. The scheduler handles the concrete path.
                    return None;
                }
            }
            HirStmt::Expr { expr, .. } => return prove(*expr, &aliases),
            HirStmt::Return { value, .. } => return prove((*value)?, &aliases),
            _ => return None,
        }
    }
    prove(body.final_expr?, &aliases)
}
