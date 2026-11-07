use std::collections::BTreeMap;

use etas_effects::{EffectFacts, EffectSummary, TaskCallFact, TaskCallKind};
use etas_hir::{HirExpr, HirExprId, ScopeId};

use super::TaskBranchShape;

pub(super) struct BranchSource {
    pub expr: HirExprId,
    pub effects: etas_effects::EffectRow,
    pub behavior: EffectSummary,
    pub shape: TaskBranchShape,
    pub incomplete: bool,
}

pub(super) fn task_scopes<'a>(
    calls: impl Iterator<Item = &'a TaskCallFact>,
) -> BTreeMap<ScopeId, Vec<&'a TaskCallFact>> {
    let mut owners = BTreeMap::<_, Vec<_>>::new();
    for call in calls {
        owners.entry(call.owner_scope).or_default().push(call);
    }
    for calls in owners.values_mut() {
        calls.sort_by_key(|call| call.call);
    }
    owners
}

pub(super) fn task_branches(
    hir: &etas_hir::HirProgram,
    effects: &EffectFacts,
    calls: &[&TaskCallFact],
) -> Vec<BranchSource> {
    let mut branches = Vec::new();
    for call in calls {
        match call.kind {
            TaskCallKind::Start => {
                let expr = call.job.unwrap_or(call.call);
                branches.push(branch_source(
                    hir,
                    effects,
                    call,
                    expr,
                    TaskBranchShape::ExplicitStart,
                    false,
                ));
            }
            TaskCallKind::Join
            | TaskCallKind::TryJoin
            | TaskCallKind::Collect
            | TaskCallKind::Race => {
                let Some(argument) = call_argument(hir, call.call, 0) else {
                    branches.push(branch_source(
                        hir,
                        effects,
                        call,
                        call.call,
                        TaskBranchShape::Dynamic,
                        true,
                    ));
                    continue;
                };
                match &hir.exprs[argument] {
                    HirExpr::Tuple { elems, .. } => branches.extend(elems.iter().map(|expr| {
                        branch_source(
                            hir,
                            effects,
                            call,
                            *expr,
                            TaskBranchShape::TupleElement,
                            false,
                        )
                    })),
                    HirExpr::Array { elems, .. } => branches.extend(elems.iter().map(|expr| {
                        branch_source(
                            hir,
                            effects,
                            call,
                            *expr,
                            TaskBranchShape::ArrayElement,
                            false,
                        )
                    })),
                    _ => branches.push(branch_source(
                        hir,
                        effects,
                        call,
                        argument,
                        TaskBranchShape::Dynamic,
                        true,
                    )),
                }
            }
            TaskCallKind::MapConcurrent => {
                let expr = call.job.unwrap_or(call.call);
                branches.push(branch_source(
                    hir,
                    effects,
                    call,
                    expr,
                    TaskBranchShape::MapperInvocation,
                    false,
                ));
            }
            TaskCallKind::Run
            | TaskCallKind::Await
            | TaskCallKind::Requeue
            | TaskCallKind::Park => {}
        }
    }
    branches
}

fn branch_source(
    hir: &etas_hir::HirProgram,
    effects: &EffectFacts,
    call: &TaskCallFact,
    expr: HirExprId,
    shape: TaskBranchShape,
    incomplete: bool,
) -> BranchSource {
    let latent = effects.latent_effects.get(&expr);
    let item_summary = match &hir.exprs[expr] {
        HirExpr::Path(path) => match path.resolution {
            etas_hir::ResolveResult::Resolved(symbol) => hir
                .canonical_source_symbol(symbol)
                .and_then(|symbol| hir.symbols.get(symbol))
                .and_then(|symbol| symbol.defining_item)
                .and_then(|item| effects.item_effects.get(&item)),
            _ => None,
        },
        _ => None,
    };
    let behavior = item_summary
        .or_else(|| latent.map(|fact| &fact.summary))
        .unwrap_or(&call.behavior)
        .clone();
    BranchSource {
        expr,
        effects: item_summary
            .map(|summary| summary.escaping_effects.clone())
            .or_else(|| latent.map(|fact| fact.inferred.clone()))
            .unwrap_or_else(|| call.effects.clone()),
        behavior,
        shape,
        incomplete,
    }
}

fn call_argument(hir: &etas_hir::HirProgram, call: HirExprId, index: usize) -> Option<HirExprId> {
    let argument = match &hir.exprs[call] {
        HirExpr::Call { args, .. } => args.get(index),
        HirExpr::MethodCall { receiver, args, .. } => {
            if index == 0 {
                return Some(*receiver);
            }
            args.get(index - 1)
        }
        _ => None,
    }?;
    Some(match argument {
        etas_hir::HirArg::Positional(expr) | etas_hir::HirArg::Named { value: expr, .. } => *expr,
    })
}
