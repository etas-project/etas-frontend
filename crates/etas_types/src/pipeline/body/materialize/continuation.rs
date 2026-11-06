use std::collections::HashSet;

use etas_core::{Diagnostic, TypeDiagnosticCode};
use etas_hir::{HirArg, HirBodyKind, HirBodyRef, HirExpr, HirExprId, HirOwner, ResolveResult};

use crate::pipeline::{body::state::BodyPipelineState, context::TypePipelineContext};

pub(super) fn validate_uses(ctx: &mut TypePipelineContext<'_>, state: &BodyPipelineState) {
    if state.provisional.continuation_bindings.is_empty() {
        return;
    }

    let view = ctx
        .hir_view
        .clone()
        .unwrap_or_else(|| etas_hir::HirTreeView::new(ctx.hir));
    let permitted = state
        .solver_report
        .std_intrinsic_calls
        .iter()
        .filter_map(|(call, fact)| continuation_argument(ctx.hir, *call, fact.intrinsic))
        .collect::<HashSet<_>>();

    for (expr, value) in ctx.hir.exprs.iter() {
        let HirExpr::Path(path) = value else {
            continue;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            continue;
        };
        let Some(binding) = state.provisional.continuation_bindings.get(&symbol) else {
            continue;
        };
        if permitted.contains(&expr)
            && enclosing_handler_arm(view.index(), expr) == Some(binding.arm)
        {
            continue;
        }

        let name = ctx
            .hir
            .symbols
            .get(symbol)
            .map_or("continuation", |symbol| symbol.name.as_str());
        ctx.diagnostics.push(Diagnostic::type_check(
            TypeDiagnosticCode::InvalidContinuationUse,
            path.span,
            format!(
                "continuation `{name}` is affine and may only be consumed directly by std.task scheduler requeue or park in this handler arm"
            ),
        ));
    }
}

pub(super) fn record_task_owners(ctx: &mut TypePipelineContext<'_>, state: &mut BodyPipelineState) {
    let view = ctx
        .hir_view
        .clone()
        .unwrap_or_else(|| etas_hir::HirTreeView::new(ctx.hir));
    let run_callbacks = state
        .solver_report
        .std_intrinsic_calls
        .iter()
        .filter(|(_, fact)| fact.intrinsic == etas_std::intrinsic::task::RUN)
        .filter_map(|(call, _)| first_argument(ctx.hir, *call))
        .collect::<HashSet<_>>();

    let calls = state
        .solver_report
        .std_intrinsic_calls
        .iter()
        .map(|(expr, fact)| (*expr, fact.intrinsic))
        .collect::<Vec<_>>();
    for (expr, intrinsic) in calls {
        if intrinsic == etas_std::intrinsic::task::RUN {
            if let Some(ty) = expression_type(state, expr)
                && contains_scoped_handle(ctx.interner.store(), ty, &mut HashSet::new())
            {
                ctx.diagnostics.push(Diagnostic::type_check(
                    TypeDiagnosticCode::InvalidTaskScope,
                    ctx.hir.exprs[expr].span(&ctx.hir.blocks),
                    "std.task.run cannot return a Task, Scheduler, or Continuation owned by its scope",
                ));
            }
            continue;
        }
        if intrinsic != etas_std::intrinsic::task::START {
            continue;
        }
        let Some(owner_scope) = task_owner_scope(view.index(), ctx.hir, expr, &run_callbacks)
        else {
            ctx.diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::InvalidTaskScope,
                ctx.hir.exprs[expr].span(&ctx.hir.blocks),
                "std.task.start must be lexically owned by a std.task.run callback",
            ));
            continue;
        };
        let Some(ty) = expression_type(state, expr) else {
            continue;
        };
        let Some(crate::Type::Task(task)) = ctx.interner.store().get(ty) else {
            continue;
        };
        state.provisional.task_owners.insert(
            expr,
            crate::TaskOwnerFact {
                owner_scope,
                output: task.output,
                effects: task.effects.clone(),
            },
        );
    }
}

fn first_argument(hir: &etas_hir::HirProgram, call: HirExprId) -> Option<HirExprId> {
    match hir.exprs.get(call)? {
        HirExpr::Call { args, .. } => argument(args, 0),
        _ => None,
    }
}

fn expression_type(state: &BodyPipelineState, expr: HirExprId) -> Option<crate::TypeId> {
    state
        .provisional
        .expr_types
        .get(&expr)
        .copied()
        .or_else(|| state.solver_report.inferred_expr_types.get(&expr).copied())
}

fn task_owner_scope(
    index: &etas_hir::HirTreeIndex,
    hir: &etas_hir::HirProgram,
    expr: HirExprId,
    run_callbacks: &HashSet<HirExprId>,
) -> Option<etas_hir::ScopeId> {
    let mut body = body_containing_expr(index, expr)?;
    loop {
        body = match body.kind {
            HirBodyKind::Lambda(lambda) => {
                if run_callbacks.contains(&lambda) {
                    let HirExpr::Lambda { scope, .. } = hir.exprs[lambda] else {
                        return None;
                    };
                    return Some(scope);
                }
                body_containing_expr(index, lambda)?
            }
            HirBodyKind::HandlerArm(arm) => {
                let HirOwner::Expr(handler) = *index.handler_arm_owner.get(&arm)? else {
                    return None;
                };
                body_containing_expr(index, handler)?
            }
            HirBodyKind::MatchArm { expr, .. } | HirBodyKind::StageLimit { expr, .. } => {
                body_containing_expr(index, expr)?
            }
            HirBodyKind::Flow
            | HirBodyKind::Tool
            | HirBodyKind::Agent
            | HirBodyKind::TopLevelLet
            | HirBodyKind::ImplFlow(_)
            | HirBodyKind::AnnotationArg { .. } => return None,
        };
    }
}

fn contains_scoped_handle(
    store: &crate::TypeStore,
    ty: crate::TypeId,
    visited: &mut HashSet<crate::TypeId>,
) -> bool {
    if !visited.insert(ty) {
        return false;
    }
    match store.get(ty) {
        Some(
            crate::Type::Task(_) | crate::Type::SchedulerHandle | crate::Type::Continuation(_),
        ) => true,
        Some(
            crate::Type::Array(inner)
            | crate::Type::List(inner)
            | crate::Type::Set(inner)
            | crate::Type::Slice(inner)
            | crate::Type::Option(inner)
            | crate::Type::Message(inner)
            | crate::Type::Schema(inner)
            | crate::Type::MemorySelection(inner)
            | crate::Type::MemoryRegion(inner),
        ) => contains_scoped_handle(store, *inner, visited),
        Some(crate::Type::Range { index }) => contains_scoped_handle(store, *index, visited),
        Some(crate::Type::Map { key, value } | crate::Type::Store { key, value }) => {
            contains_scoped_handle(store, *key, visited)
                || contains_scoped_handle(store, *value, visited)
        }
        Some(crate::Type::Result { ok, err }) => {
            contains_scoped_handle(store, *ok, visited)
                || contains_scoped_handle(store, *err, visited)
        }
        Some(crate::Type::Tuple(elements)) => elements
            .iter()
            .any(|element| contains_scoped_handle(store, *element, visited)),
        Some(crate::Type::Record(record)) => record
            .fields
            .iter()
            .any(|field| contains_scoped_handle(store, field.ty, visited)),
        Some(crate::Type::Function(flow)) => {
            flow.input
                .iter()
                .any(|input| contains_scoped_handle(store, *input, visited))
                || contains_scoped_handle(store, flow.output, visited)
        }
        Some(crate::Type::Trust { inner, .. } | crate::Type::Refined { base: inner, .. }) => {
            contains_scoped_handle(store, *inner, visited)
        }
        Some(crate::Type::ResourceHandle(crate::ResourceHandleType::MemoryRegion { schema })) => {
            contains_scoped_handle(store, *schema, visited)
        }
        Some(crate::Type::ResourceHandle(crate::ResourceHandleType::ExternalTool {
            signature,
        })) => contains_scoped_handle(store, *signature, visited),
        Some(crate::Type::ResourceHandle(crate::ResourceHandleType::Other { args, .. }))
        | Some(crate::Type::Applied { args, .. }) => args
            .iter()
            .any(|arg| contains_scoped_handle(store, *arg, visited)),
        Some(crate::Type::Handler(handler)) => handler
            .result
            .is_some_and(|result| contains_scoped_handle(store, result, visited)),
        _ => false,
    }
}

fn continuation_argument(
    hir: &etas_hir::HirProgram,
    call: HirExprId,
    intrinsic: u32,
) -> Option<HirExprId> {
    let free_index = match intrinsic {
        etas_std::intrinsic::task::REQUEUE => 1,
        etas_std::intrinsic::task::PARK => 2,
        _ => return None,
    };
    match hir.exprs.get(call)? {
        HirExpr::Call { args, .. } => argument(args, free_index),
        HirExpr::MethodCall { args, .. } => argument(args, free_index - 1),
        _ => None,
    }
}

fn argument(args: &[HirArg], index: usize) -> Option<HirExprId> {
    match args.get(index)? {
        HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => Some(*expr),
    }
}

fn enclosing_handler_arm(
    index: &etas_hir::HirTreeIndex,
    expr: HirExprId,
) -> Option<etas_hir::HirHandlerArmId> {
    let mut body = body_containing_expr(index, expr)?;
    loop {
        match body.kind {
            HirBodyKind::HandlerArm(arm) => return Some(arm),
            HirBodyKind::MatchArm { expr, .. } | HirBodyKind::StageLimit { expr, .. } => {
                body = body_containing_expr(index, expr)?;
            }
            HirBodyKind::Lambda(_) => return None,
            HirBodyKind::Flow
            | HirBodyKind::Tool
            | HirBodyKind::Agent
            | HirBodyKind::TopLevelLet
            | HirBodyKind::ImplFlow(_)
            | HirBodyKind::AnnotationArg { .. } => return None,
        }
    }
}

fn body_containing_expr(index: &etas_hir::HirTreeIndex, expr: HirExprId) -> Option<HirBodyRef> {
    let mut owner = *index.expr_owner.get(&expr)?;
    loop {
        owner = match owner {
            HirOwner::Body(body) => return Some(body),
            HirOwner::Expr(expr) => *index.expr_owner.get(&expr)?,
            HirOwner::Block(block) => *index.block_owner.get(&block)?,
            HirOwner::Stmt(stmt) => {
                let block = *index.stmt_block.get(&stmt)?;
                *index.block_owner.get(&block)?
            }
            HirOwner::HandlerArm(_)
            | HirOwner::Item(_)
            | HirOwner::Module(_)
            | HirOwner::Pattern(_) => return None,
        };
    }
}
