//! Projections of the selected source execution declarations. These expose
//! task result/effect parameters without replacing a source record's identity.
use crate::{EffectRowRef, KnownStdTypes, Type, TypeId, TypeStore};

/// Resolve an execution primitive called through a source name or immutable
/// callable alias. Runtime values still use the ordinary checked call ABI.
pub fn call_target(
    hir: &etas_hir::HirProgram,
    facts: &crate::TypeFacts,
    call: etas_hir::HirExprId,
) -> Option<etas_std::StdExecutionRole> {
    let etas_hir::HirExpr::Call { callee, .. } = hir.exprs.get(call)? else {
        return None;
    };
    let symbol = callable_symbol(hir, *callee)?;
    if let etas_hir::SymbolDef::Item { item } = &hir.symbols.get(symbol)?.def
        && let Some(etas_hir::HirItem::Flow(etas_hir::HirFlowDecl {
            body: etas_hir::HirFlowBody::Runtime { role, .. },
            ..
        })) = hir.items.get(*item)
    {
        return Some(*role);
    }
    facts.source_execution_roles.get(&symbol).copied()
}

pub fn callable_symbol(
    hir: &etas_hir::HirProgram,
    mut expr: etas_hir::HirExprId,
) -> Option<etas_hir::SymbolId> {
    let mut visited = std::collections::HashSet::new();
    loop {
        if !visited.insert(expr) {
            return None;
        }
        let etas_hir::HirExpr::Path(path) = hir.exprs.get(expr)? else {
            return None;
        };
        let etas_hir::ResolveResult::Resolved(symbol) = path.resolution else {
            return None;
        };
        let symbol = hir.canonical_source_symbol(symbol)?;
        match &hir.symbols.get(symbol)?.def {
            etas_hir::SymbolDef::Local {
                initializer: Some(initializer),
                binding,
                ..
            } if !matches!(hir.stmts.get(*binding), Some(etas_hir::HirStmt::Var { .. })) => {
                expr = *initializer
            }
            etas_hir::SymbolDef::TopLevelLet { initializer, .. } => expr = *initializer,
            etas_hir::SymbolDef::PatternBinding {
                initializer: Some(initializer),
                ..
            } => expr = *initializer,
            _ => return Some(symbol),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TaskView<'a> {
    pub output: TypeId,
    pub effects: &'a EffectRowRef,
}

pub fn task<'a>(store: &'a TypeStore, known: &KnownStdTypes, ty: TypeId) -> Option<TaskView<'a>> {
    match store.get(ty)? {
        Type::Task(task) => Some(TaskView {
            output: task.output,
            effects: &task.effects,
        }),
        Type::Applied { constructor, args }
            if known.execution.task == Some(TypeId(constructor.0)) =>
        {
            let [output, effects] = args.as_slice() else {
                return None;
            };
            let Type::Handler(row) = store.get(*effects)? else {
                return None;
            };
            Some(TaskView {
                output: *output,
                effects: &row.handled,
            })
        }
        _ => None,
    }
}

pub fn continuation_output(store: &TypeStore, known: &KnownStdTypes, ty: TypeId) -> Option<TypeId> {
    match store.get(ty)? {
        Type::Continuation(value) => Some(value.action_output),
        Type::Applied { constructor, args }
            if known.execution.continuation == Some(TypeId(constructor.0)) =>
        {
            match args.as_slice() {
                [output] => Some(*output),
                _ => None,
            }
        }
        _ => None,
    }
}

pub fn is_scheduler(store: &TypeStore, known: &KnownStdTypes, ty: TypeId) -> bool {
    matches!(store.get(ty), Some(Type::SchedulerHandle)) || known.execution.scheduler == Some(ty)
}

pub fn is_scoped_handle(store: &TypeStore, known: &KnownStdTypes, ty: TypeId) -> bool {
    task(store, known, ty).is_some()
        || continuation_output(store, known, ty).is_some()
        || is_scheduler(store, known, ty)
        || [
            known.execution.task_handle,
            known.execution.scheduler_state,
            known.execution.continuation_state,
        ]
        .into_iter()
        .any(|capability| capability == Some(ty))
}
