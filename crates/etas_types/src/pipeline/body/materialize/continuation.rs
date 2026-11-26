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
    let mut permitted = state
        .solver_report
        .std_intrinsic_calls
        .iter()
        .filter_map(|(call, fact)| continuation_argument(ctx.hir, *call, fact.intrinsic))
        .collect::<HashSet<_>>();
    permitted.extend(
        state
            .solver_report
            .checked_method_calls
            .iter()
            .filter_map(|(call, fact)| {
                let slot = match crate::method::execution_role(
                    ctx.hir,
                    ctx.signature_base
                        .map_or(&ctx.signature_facts, |base| &base.facts),
                    &fact.target,
                )? {
                    etas_std::StdExecutionRole::Requeue => 1,
                    etas_std::StdExecutionRole::Park => 2,
                    _ => return None,
                };
                crate::method::argument_expression(ctx.hir, *call, fact, slot)
            }),
    );

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
    use etas_std::intrinsic::task::{RUN, START};

    let hir = ctx.hir;
    let facts = ctx
        .signature_base
        .map_or(&ctx.signature_facts, |base| &base.facts);
    let report = &state.solver_report;
    let mut run_callbacks = HashSet::new();
    let mut calls = Vec::new();
    for (&call, fact) in &report.std_intrinsic_calls {
        if matches!(fact.intrinsic, RUN | START) {
            calls.push((call, fact.intrinsic));
            if fact.intrinsic == RUN {
                run_callbacks.extend(first_argument(hir, call));
            }
        }
    }
    for (&call, method) in &report.checked_method_calls {
        if crate::method::execution_role(hir, facts, &method.target)
            == Some(etas_std::StdExecutionRole::ScopeEnter)
        {
            calls.push((call, RUN));
            run_callbacks.extend(crate::method::argument_expression(hir, call, method, 1));
        }
    }

    let provisional = &state.provisional.expr_types;
    let typed = provisional.keys().chain(
        report
            .inferred_expr_types
            .keys()
            .filter(|id| !provisional.contains_key(id)),
    );
    for &call in typed {
        if crate::source_execution::call_target(hir, facts, call)
            == Some(etas_std::StdExecutionRole::TaskStart)
        {
            calls.push((call, START));
        }
        if !crate::task_scope::arguments(hir, facts, call, report.checked_method_calls.get(&call))
            .is_empty()
        {
            calls.push((call, RUN));
        }
    }
    calls.sort_unstable();
    calls.dedup();
    // A start can be inside a callback whose enclosing scope call belongs to
    // another body. Retain those source scopes when looking up its owner.
    if calls.iter().any(|(_, intrinsic)| *intrinsic == START) {
        run_callbacks.extend(hir.exprs.iter().flat_map(|(call, _)| {
            crate::task_scope::arguments(hir, facts, call, report.checked_method_calls.get(&call))
        }));
    }
    for (expr, intrinsic) in calls {
        if intrinsic == etas_std::intrinsic::task::RUN {
            if let Some(ty) = expression_type(state, expr)
                && contains_scoped_handle(
                    ctx.interner.store(),
                    ctx.signature_base
                        .map_or(&ctx.signature_facts, |base| &base.facts),
                    ty,
                    &mut HashSet::new(),
                )
            {
                ctx.diagnostics.push(Diagnostic::type_check(
                    TypeDiagnosticCode::InvalidTaskScope,
                    ctx.hir.exprs[expr].span(&ctx.hir.blocks),
                    "a task scope cannot return a Task, Scheduler, or Continuation owned by that scope",
                ));
            }
            continue;
        }
        let view = ctx
            .hir_view
            .get_or_insert_with(|| etas_hir::HirTreeView::new(hir));
        let Some(owner_scope) = task_owner_scope(view.index(), hir, expr, &run_callbacks)
            .or_else(|| ctx.hir.native_binding_owner(expr).map(|flow| flow.scope))
        else {
            ctx.diagnostics.push(Diagnostic::type_check(
                TypeDiagnosticCode::InvalidTaskScope,
                ctx.hir.exprs[expr].span(&ctx.hir.blocks),
                "task start requires a callback proven to execute within a managed task scope",
            ));
            continue;
        };
        let Some(ty) = expression_type(state, expr) else {
            continue;
        };
        let Some(task) =
            crate::source_execution::task(ctx.interner.store(), ctx.known_std_types(), ty)
        else {
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
            HirBodyKind::MatchArm { expr, .. } => body_containing_expr(index, expr)?,
            HirBodyKind::Flow
            | HirBodyKind::Tool
            | HirBodyKind::Agent
            | HirBodyKind::TopLevelLet
            | HirBodyKind::ImplFlow(_)
            | HirBodyKind::AnnotationArg { .. } => return None,
            HirBodyKind::ParameterDefault { .. } | HirBodyKind::FieldAnnotation { .. } => {
                return None;
            }
        };
    }
}

fn contains_scoped_handle(
    store: &crate::TypeStore,
    facts: &crate::TypeFacts,
    ty: crate::TypeId,
    visited: &mut HashSet<crate::TypeId>,
) -> bool {
    if !visited.insert(ty) {
        return false;
    }
    if crate::source_execution::is_scoped_handle(store, &facts.known_std_types, ty) {
        return true;
    }
    match store.get(ty) {
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
        ) => contains_scoped_handle(store, facts, *inner, visited),
        Some(crate::Type::Range { index }) => contains_scoped_handle(store, facts, *index, visited),
        Some(crate::Type::Map { key, value } | crate::Type::Store { key, value }) => {
            contains_scoped_handle(store, facts, *key, visited)
                || contains_scoped_handle(store, facts, *value, visited)
        }
        Some(crate::Type::Result { ok, err }) => {
            contains_scoped_handle(store, facts, *ok, visited)
                || contains_scoped_handle(store, facts, *err, visited)
        }
        Some(crate::Type::Tuple(elements)) => elements
            .iter()
            .any(|element| contains_scoped_handle(store, facts, *element, visited)),
        Some(crate::Type::Record(record)) => record
            .fields
            .iter()
            .any(|field| contains_scoped_handle(store, facts, field.ty, visited)),
        Some(crate::Type::Function(flow)) => {
            flow.input
                .iter()
                .any(|input| contains_scoped_handle(store, facts, *input, visited))
                || contains_scoped_handle(store, facts, flow.output, visited)
        }
        Some(crate::Type::Trust { inner, .. } | crate::Type::Refined { base: inner, .. }) => {
            contains_scoped_handle(store, facts, *inner, visited)
        }
        Some(crate::Type::ResourceHandle(crate::ResourceHandleType::MemoryRegion { schema })) => {
            contains_scoped_handle(store, facts, *schema, visited)
        }
        Some(crate::Type::ResourceHandle(crate::ResourceHandleType::ExternalTool {
            signature,
        })) => contains_scoped_handle(store, facts, *signature, visited),
        Some(crate::Type::Nominal(owner)) => owner
            .representation
            .is_some_and(|layout| contains_scoped_handle(store, facts, layout, visited)),
        Some(crate::Type::Enum(_)) => facts.enum_layouts.get(&ty).is_some_and(|layout| {
            layout
                .variants
                .iter()
                .flat_map(|variant| &variant.fields)
                .any(|field| contains_scoped_handle(store, facts, *field, visited))
        }),
        Some(crate::Type::Applied { constructor, args }) => {
            contains_scoped_handle(store, facts, crate::TypeId(constructor.0), visited)
                || args
                    .iter()
                    .any(|arg| contains_scoped_handle(store, facts, *arg, visited))
        }
        Some(crate::Type::ResourceHandle(crate::ResourceHandleType::Other { args, .. })) => args
            .iter()
            .any(|arg| contains_scoped_handle(store, facts, *arg, visited)),
        Some(crate::Type::Handler(handler)) => handler
            .result
            .is_some_and(|result| contains_scoped_handle(store, facts, result, visited)),
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
            HirBodyKind::MatchArm { expr, .. } => {
                body = body_containing_expr(index, expr)?;
            }
            HirBodyKind::Lambda(_) => return None,
            HirBodyKind::Flow
            | HirBodyKind::Tool
            | HirBodyKind::Agent
            | HirBodyKind::TopLevelLet
            | HirBodyKind::ImplFlow(_)
            | HirBodyKind::AnnotationArg { .. } => return None,
            HirBodyKind::ParameterDefault { .. } | HirBodyKind::FieldAnnotation { .. } => {
                return None;
            }
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
