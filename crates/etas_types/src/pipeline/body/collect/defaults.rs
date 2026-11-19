use crate::{
    AssignabilityReason, ConstraintOrigin, SymbolTypeFact, TypeConstraint,
    pipeline::{
        body::state::BodyPipelineState,
        context::{BodyCollectContext, TypePipelineContext},
    },
};
use etas_hir::{HirExpr, HirTreeChild, ResolveResult, SymbolDef};

pub(super) fn collect<E>(
    ctx: &mut TypePipelineContext<'_>,
    state: &mut BodyPipelineState,
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<(), E> {
    let mut defaults = ctx
        .hir
        .parameter_defaults
        .iter()
        .filter_map(|(parameter, expr)| {
            matches!(ctx.hir.symbols.get(*parameter).map(|symbol| &symbol.def),
            Some(SymbolDef::Param { owner, .. }) if *owner == state.item)
            .then_some((*parameter, *expr))
        })
        .collect::<Vec<_>>();
    defaults.sort_by_key(|(parameter, _)| *parameter);
    for (parameter, expr) in defaults {
        checkpoint()?;
        let Some(SymbolTypeFact::Param { ty }) = ctx.symbol_fact(parameter) else {
            ctx.diagnostics.push(etas_core::Diagnostic::type_check(
                etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                ctx.hir.exprs[expr].span(&ctx.hir.blocks),
                "parameter default has no checked declared parameter type",
            ));
            continue;
        };
        let expected = *ty;
        let span = ctx.hir.exprs[expr].span(&ctx.hir.blocks);
        validate_bindings(ctx, parameter, expr);
        let enclosing_return = state.expected_return.replace(expected);
        let mut body = BodyCollectContext {
            ctx,
            state,
            execution: checkpoint,
        };
        let actual = super::expr::collect_expr(&mut body, expr, Some(expected))?;
        body.emit(TypeConstraint::Assignable {
            from: actual,
            to: expected,
            origin: ConstraintOrigin { span },
            reason: AssignabilityReason::Argument,
        });
        body.state.expected_return = enclosing_return;
    }
    Ok(())
}

fn validate_bindings(
    ctx: &mut TypePipelineContext<'_>,
    parameter: etas_hir::SymbolId,
    root: etas_hir::HirExprId,
) {
    let SymbolDef::Param {
        owner, param_index, ..
    } = ctx
        .hir
        .symbols
        .get(parameter)
        .expect("default parameter")
        .def
    else {
        unreachable!()
    };
    let parameters = ctx
        .hir
        .parameter_list(parameter)
        .expect("authored default parameter list");
    let receiver = match &ctx.hir.items[owner] {
        etas_hir::HirItem::Flow(flow) => flow.receiver_mode != etas_hir::HirReceiverMode::None,
        etas_hir::HirItem::Spec(spec) => spec.items.iter().any(|member| matches!(member,
            etas_hir::HirSpecItem::FlowSignature(signature) if signature.params.contains(&parameter) && signature.receiver_mode != etas_hir::HirReceiverMode::None)),
        _ => false,
    };
    let mut pending = vec![HirTreeChild::Expr(root)];
    let mut paths = Vec::new();
    let mut local_statements = std::collections::HashSet::new();
    while let Some(child) = pending.pop() {
        match child {
            HirTreeChild::Expr(expr) => {
                if matches!(ctx.hir.exprs[expr], HirExpr::Path(_)) {
                    paths.push(expr);
                }
                match &ctx.hir.exprs[expr] {
                    HirExpr::Lambda {
                        body: etas_hir::HirLambdaBody::Expr(value),
                        ..
                    } => pending.push(HirTreeChild::Expr(*value)),
                    HirExpr::Lambda {
                        body: etas_hir::HirLambdaBody::Block(block),
                        ..
                    } => pending.push(HirTreeChild::Block(*block)),
                    _ => etas_hir::for_each_expr_child(ctx.hir, expr, |child| pending.push(child)),
                }
            }
            HirTreeChild::Block(block) => {
                pending.extend(
                    ctx.hir.blocks[block]
                        .stmts
                        .iter()
                        .copied()
                        .map(HirTreeChild::Stmt),
                );
                pending.extend(ctx.hir.blocks[block].final_expr.map(HirTreeChild::Expr));
            }
            HirTreeChild::Stmt(stmt) => {
                local_statements.insert(stmt);
                etas_hir::for_each_stmt_child(ctx.hir, stmt, |child| pending.push(child));
            }
            HirTreeChild::HandlerArm(arm) => {
                pending.push(HirTreeChild::Block(ctx.hir.handler_arms[arm].body))
            }
            HirTreeChild::Pattern(_) => (),
        }
    }
    for expr in paths {
        let HirExpr::Path(path) = &ctx.hir.exprs[expr] else {
            unreachable!()
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            continue;
        };
        let Some(data) = ctx.hir.symbols.get(symbol) else {
            continue;
        };
        let invalid = match data.def {
            SymbolDef::Param {
                param_index: index, ..
            } => {
                parameters.contains(&symbol)
                    && (index >= param_index || (receiver && parameters.first() == Some(&symbol)))
            }
            SymbolDef::Local { binding, .. } => {
                !local_statements.contains(&binding)
                    && matches!(
                        ctx.hir.stmts.get(binding),
                        Some(etas_hir::HirStmt::Var { .. })
                    )
            }
            _ => false,
        };
        if invalid {
            ctx.diagnostics.push(etas_core::Diagnostic::type_check(etas_core::TypeDiagnosticCode::TypeMismatch, path.span,
                "a parameter default may use preceding ordinary parameters and immutable definition-site bindings, but not a receiver, later parameter or mutable outer binding"));
        }
    }
}
