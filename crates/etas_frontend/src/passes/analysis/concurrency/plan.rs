use std::collections::HashMap;

use etas_effects::{
    EffectFacts, EffectRegistry, InterpreterSupport, LimitKind, LimitValue, RequirementFact,
    TaskCallFact, TaskCallKind,
};
use etas_hir::{HirArg, HirExpr, HirPat, HirStmt, ResolveResult};

use super::{
    AwaitSiteId, BranchFact, ConcurrencyFacts, ConcurrencyPlanFact, ConcurrencySiteFact,
    ContinuationTransferFact, ContinuationTransferId, SchedulingDecision, TaskAwaitFact,
    TaskScopeFact, TaskSiteId, TaskStartFact, conflict, ownership,
};

pub(super) fn analyze(
    hir: &etas_hir::HirProgram,
    effects: &EffectFacts,
    registry: &EffectRegistry,
) -> ConcurrencyFacts {
    let owners = ownership::task_scopes(effects.task_calls.values());
    let mut scope_facts = HashMap::new();
    let mut sites = HashMap::new();
    let mut starts = HashMap::new();
    let mut plans = HashMap::new();

    for (owner_scope, calls) in owners {
        let task_calls = ownership::task_branches(&calls);
        let branches = task_calls
            .iter()
            .map(|call| BranchFact {
                task_site: call.call,
                effects: call.effects.clone(),
                actions: call.behavior.action_trace.action_set(),
            })
            .collect::<Vec<_>>();
        let incomplete = task_calls.iter().any(|call| {
            call.effects.open.is_some()
                || call.behavior.requested_actions.open.is_some()
                || call.behavior.action_trace.has_parameter_calls()
        });
        let rejected = calls
            .iter()
            .any(|call| matches!(call.behavior.support, InterpreterSupport::Rejected(_)));
        let decision = if rejected {
            SchedulingDecision::Reject
        } else {
            conflict::scheduling_decision(&branches, registry, incomplete)
        };
        let concurrency_limit = calls
            .iter()
            .filter_map(|call| concurrency_limit(call))
            .min();
        let scope = TaskScopeFact {
            owner_scope,
            sites: calls.iter().map(|call| call.call).collect(),
            branches: branches.clone(),
            decision: decision.clone(),
            concurrency_limit,
        };
        let stable_branches = task_calls
            .iter()
            .map(|call| task_site_id(call_span(hir, call.call)))
            .collect::<Vec<_>>();
        plans.insert(
            owner_scope,
            ConcurrencyPlanFact {
                owner_scope,
                branches: stable_branches,
                decision: decision.clone(),
                concurrency_limit,
            },
        );
        for call in calls {
            let scheduling = match call.kind {
                TaskCallKind::Await | TaskCallKind::Requeue | TaskCallKind::Park => {
                    SchedulingDecision::Concurrent
                }
                _ => decision.clone(),
            };
            sites.insert(
                call.call,
                ConcurrencySiteFact {
                    kind: call.kind,
                    owner_scope,
                    decision: scheduling,
                    branches: branches.clone(),
                    concurrency_limit,
                },
            );
            if call.kind == TaskCallKind::Start {
                let span = call_span(hir, call.call);
                starts.insert(
                    call.call,
                    TaskStartFact {
                        id: task_site_id(span),
                        call: call.call,
                        owner_scope,
                        span,
                        output: call.output,
                        effects: call.effects.clone(),
                        behavior: call.behavior.clone(),
                        decision: decision.clone(),
                        concurrency_limit,
                    },
                );
            }
        }
        scope_facts.insert(owner_scope, scope);
    }

    let start_symbols = start_bindings(hir, &starts);
    let awaits = effects
        .task_calls
        .values()
        .filter(|call| call.kind == TaskCallKind::Await)
        .map(|call| {
            let span = call_span(hir, call.call);
            let task = first_argument(hir, call.call)
                .and_then(|argument| resolve_task_start(hir, argument, &starts, &start_symbols));
            (
                call.call,
                TaskAwaitFact {
                    id: await_site_id(span),
                    call: call.call,
                    owner_scope: call.owner_scope,
                    span,
                    output: call.output,
                    task,
                },
            )
        })
        .collect();
    let continuation_transfers = effects
        .continuation_transfers
        .values()
        .map(|transfer| {
            let span = call_span(hir, transfer.call);
            (
                transfer.call,
                ContinuationTransferFact {
                    id: continuation_transfer_id(span),
                    call: transfer.call,
                    binder: transfer.binder,
                    span,
                    target: transfer.target.clone(),
                },
            )
        })
        .collect();

    ConcurrencyFacts {
        sites,
        owners: scope_facts,
        starts,
        awaits,
        continuation_transfers,
        plans,
    }
}

fn call_span(hir: &etas_hir::HirProgram, call: etas_hir::HirExprId) -> etas_core::Span {
    hir.exprs[call].span(&hir.blocks)
}

fn task_site_id(span: etas_core::Span) -> TaskSiteId {
    TaskSiteId {
        source: span.source,
        start: span.range.start.0,
        end: span.range.end.0,
    }
}

fn await_site_id(span: etas_core::Span) -> AwaitSiteId {
    AwaitSiteId {
        source: span.source,
        start: span.range.start.0,
        end: span.range.end.0,
    }
}

fn continuation_transfer_id(span: etas_core::Span) -> ContinuationTransferId {
    ContinuationTransferId {
        source: span.source,
        start: span.range.start.0,
        end: span.range.end.0,
    }
}

fn start_bindings(
    hir: &etas_hir::HirProgram,
    starts: &HashMap<etas_hir::HirExprId, TaskStartFact>,
) -> HashMap<etas_hir::SymbolId, TaskSiteId> {
    let mut bindings = HashMap::new();
    for (_, statement) in hir.stmts.iter() {
        let (pat, value) = match statement {
            HirStmt::Let { pat, value, .. } | HirStmt::Var { pat, value, .. } => (*pat, *value),
            _ => continue,
        };
        let Some(start) = starts.get(&value) else {
            continue;
        };
        if let HirPat::Binding { symbol, .. } = hir.pats[pat] {
            bindings.insert(symbol, start.id);
        }
    }
    bindings
}

fn first_argument(
    hir: &etas_hir::HirProgram,
    call: etas_hir::HirExprId,
) -> Option<etas_hir::HirExprId> {
    match &hir.exprs[call] {
        HirExpr::Call { args, .. } => args.first().map(argument_expr),
        HirExpr::MethodCall { receiver, .. } => Some(*receiver),
        _ => None,
    }
}

fn argument_expr(argument: &HirArg) -> etas_hir::HirExprId {
    match argument {
        HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => *expr,
    }
}

fn resolve_task_start(
    hir: &etas_hir::HirProgram,
    argument: etas_hir::HirExprId,
    starts: &HashMap<etas_hir::HirExprId, TaskStartFact>,
    bindings: &HashMap<etas_hir::SymbolId, TaskSiteId>,
) -> Option<TaskSiteId> {
    if let Some(start) = starts.get(&argument) {
        return Some(start.id);
    }
    let HirExpr::Path(path) = &hir.exprs[argument] else {
        return None;
    };
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return None;
    };
    bindings.get(&symbol).copied()
}

fn concurrency_limit(call: &TaskCallFact) -> Option<u32> {
    call.behavior.requirements.iter().find_map(|requirement| {
        let RequirementFact::Limit(limit) = requirement else {
            return None;
        };
        if limit.kind != LimitKind::Concurrency {
            return None;
        }
        let Some(LimitValue::Count(value)) = limit.value else {
            return None;
        };
        u32::try_from(value).ok()
    })
}
