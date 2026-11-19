use std::collections::{HashMap, HashSet};

use etas_hir::{HirExpr, HirItemId, HirRevision, ResolveResult, SymbolDef};
use etas_types::{SymbolTypeFact, TypeFacts};

/// A local var is stateful even when its captured use only reads it. Propagate
/// that dependency to source callers before proving evaluation repeatable.
pub(crate) fn stateful_items(
    hir: &HirRevision,
    types: &TypeFacts,
) -> Result<HashSet<HirItemId>, String> {
    let mut stateful = HashSet::new();
    for (expr, layout) in etas_hir_analysis::closures::analyze_runtime_captures(&hir.view())? {
        if layout.captures.iter().any(|symbol| {
            matches!(
                types.symbol_types.get(symbol),
                Some(SymbolTypeFact::Local { mutable: true, .. })
            )
        }) {
            if let Some(owner) = hir.index().expr_item.get(&expr) {
                stateful.insert(*owner);
            }
        }
    }
    let mut callers: HashMap<HirItemId, Vec<HirItemId>> = HashMap::new();
    for (expr, value) in hir.exprs.iter() {
        let Some(caller) = hir.index().expr_item.get(&expr) else {
            continue;
        };
        if let HirExpr::Call { callee, .. } = value {
            let indirect = match hir.exprs.get(*callee) {
                Some(HirExpr::Lambda { .. }) => false,
                Some(HirExpr::Path(path)) => match path.resolution {
                    ResolveResult::Resolved(symbol) => hir
                        .canonical_source_symbol(symbol)
                        .and_then(|symbol| hir.symbols.get(symbol))
                        .is_some_and(|symbol| {
                            matches!(
                                symbol.def,
                                SymbolDef::Param { .. }
                                    | SymbolDef::Local { .. }
                                    | SymbolDef::PatternBinding { .. }
                                    | SymbolDef::TopLevelLet { .. }
                            )
                        }),
                    _ => true,
                },
                _ => true,
            };
            if indirect {
                stateful.insert(*caller);
            }
        }
        let target = if let Some(method) = types.checked_method_calls.get(&expr) {
            match method.target {
                etas_types::method::MethodTarget::Source { callable, .. }
                | etas_types::method::MethodTarget::Agent { callable, .. } => Some(callable),
                etas_types::method::MethodTarget::BoundSpec { .. }
                | etas_types::method::MethodTarget::External { .. } => {
                    stateful.insert(*caller);
                    None
                }
                _ => None,
            }
        } else if let HirExpr::Path(path) = value {
            match path.resolution {
                ResolveResult::Resolved(symbol) => hir
                    .canonical_source_symbol(symbol)
                    .and_then(|symbol| hir.symbols.get(symbol))
                    .and_then(|symbol| match symbol.def {
                        SymbolDef::Item { item } => Some(item),
                        _ => types
                            .type_impl_methods
                            .get(&symbol.id)
                            .map(|method| method.body),
                    }),
                _ => None,
            }
        } else {
            None
        };
        if let Some(target) = target {
            callers.entry(target).or_default().push(*caller);
        }
    }
    let mut pending = stateful.iter().copied().collect::<Vec<_>>();
    while let Some(item) = pending.pop() {
        for caller in callers.get(&item).into_iter().flatten() {
            if stateful.insert(*caller) {
                pending.push(*caller);
            }
        }
    }
    Ok(stateful)
}

/// Diagnose source-visible scope transfers. Opaque higher-order environments
/// retain the same runtime binding ownership checks at their actual boundary.
pub(crate) fn escape_diagnostics(
    hir: &HirRevision,
    types: &TypeFacts,
) -> Result<Vec<etas_core::Diagnostic>, String> {
    use etas_hir::{HirNodeRef, HirOwner, HirStmt};
    let layouts = etas_hir_analysis::closures::analyze_runtime_captures(&hir.view())?;
    let scopes = etas_hir_analysis::closures::ClosureScopes::build(&hir.scopes)?;
    let view = hir.view();
    let mut diagnostics = Vec::new();
    let origins = |expr| retained_mutable_bindings(hir, types, &layouts, expr);
    let declaration = |symbol| match hir.symbols.get(symbol).map(|symbol| &symbol.def) {
        Some(SymbolDef::Local { binding, .. }) => Some(*binding),
        _ => None,
    };
    let mut report = |span| {
        diagnostics.push(etas_core::Diagnostic::type_check(
        etas_core::TypeDiagnosticCode::TypeMismatch, span,
        "lambda or Handler retaining a local var cannot leave its owning scope; capture an immutable snapshot instead",
    ))
    };
    for (stmt, value) in hir.stmts.iter() {
        let returned = match value {
            HirStmt::Return { value, .. } | HirStmt::Resume { value, .. } => *value,
            HirStmt::Finish { value, .. } => Some(*value),
            _ => None,
        };
        if let Some(value) = returned {
            let activation = capture_activation(hir, HirOwner::Stmt(stmt));
            if origins(value)
                .into_iter()
                .filter_map(declaration)
                .any(|binding| {
                    activation.is_some()
                        && capture_activation(hir, HirOwner::Stmt(binding)) == activation
                })
            {
                report(hir.exprs[value].span(&hir.blocks));
            }
        }
        if let HirStmt::Assign {
            target,
            value,
            span,
        } = value
        {
            let Some(HirExpr::Path(path)) = hir.exprs.get(*target) else {
                continue;
            };
            let ResolveResult::Resolved(target) = path.resolution else {
                continue;
            };
            let Some(target) =
                declaration(target).and_then(|stmt| view.enclosing_scope(HirNodeRef::Stmt(stmt)))
            else {
                continue;
            };
            for origin in origins(*value) {
                let Some(source) = declaration(origin)
                    .and_then(|stmt| view.enclosing_scope(HirNodeRef::Stmt(stmt)))
                else {
                    continue;
                };
                if target != source && scopes.contains(target, source)? {
                    report(*span);
                    break;
                }
            }
        }
    }
    for (_, block) in hir.blocks.iter() {
        let Some(value) = block.final_expr else {
            continue;
        };
        for origin in origins(value) {
            let Some(source) =
                declaration(origin).and_then(|stmt| view.enclosing_scope(HirNodeRef::Stmt(stmt)))
            else {
                continue;
            };
            if scopes.contains(block.scope, source)? {
                report(hir.exprs[value].span(&hir.blocks));
                break;
            }
        }
    }
    Ok(diagnostics)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CaptureActivation {
    Item(HirItemId),
    Lambda(etas_hir::HirExprId),
    Handler(etas_hir::HirHandlerArmId),
}

fn capture_activation(
    hir: &HirRevision,
    mut owner: etas_hir::HirOwner,
) -> Option<CaptureActivation> {
    use etas_hir::{HirBodyKind, HirOwner};
    let index = hir.index();
    loop {
        owner = match owner {
            HirOwner::Item(item) => return Some(CaptureActivation::Item(item)),
            HirOwner::HandlerArm(arm) => return Some(CaptureActivation::Handler(arm)),
            HirOwner::Body(body) => match body.kind {
                HirBodyKind::Lambda(expr) => return Some(CaptureActivation::Lambda(expr)),
                HirBodyKind::HandlerArm(arm) => return Some(CaptureActivation::Handler(arm)),
                HirBodyKind::MatchArm { .. } | HirBodyKind::StageLimit { .. } => {
                    *index.body_owner.get(&body)?
                }
                _ => return Some(CaptureActivation::Item(body.item)),
            },
            HirOwner::Stmt(stmt) => *index.block_owner.get(index.stmt_block.get(&stmt)?)?,
            HirOwner::Block(block) => *index.block_owner.get(&block)?,
            HirOwner::Expr(expr) => *index.expr_owner.get(&expr)?,
            HirOwner::Pattern(pat) => *index.pat_owner.get(&pat)?,
            HirOwner::Module(_) => return None,
        };
    }
}

fn retained_mutable_bindings(
    hir: &HirRevision,
    types: &TypeFacts,
    layouts: &HashMap<etas_hir::HirExprId, etas_hir_analysis::closures::ClosureCaptureLayout>,
    expr: etas_hir::HirExprId,
) -> HashSet<etas_hir::SymbolId> {
    use etas_hir::{HirFieldInit, HirStmt};
    enum Value {
        Expr(etas_hir::HirExprId),
        Binding(etas_hir::SymbolId),
    }
    let mut pending = vec![Value::Expr(expr)];
    let mut expressions = HashSet::new();
    let mut bindings = HashSet::new();
    let mut retained = HashSet::new();
    while let Some(value) = pending.pop() {
        match value {
            Value::Binding(symbol) if bindings.insert(symbol) => {
                if let Some(SymbolDef::Local {
                    binding,
                    initializer: Some(value),
                    ..
                }) = hir.symbols.get(symbol).map(|symbol| &symbol.def)
                {
                    if let HirStmt::Let { pat, .. } = hir.stmts[*binding]
                        && matches!(hir.pats[pat], etas_hir::HirPat::Binding { symbol: declared, .. } if declared == symbol)
                    {
                        pending.push(Value::Expr(*value));
                    }
                }
            }
            Value::Expr(expr) if expressions.insert(expr) => match &hir.exprs[expr] {
                HirExpr::Lambda { .. } | HirExpr::Handler { .. } => {
                    for symbol in &layouts[&expr].captures {
                        if matches!(
                            types.symbol_types.get(symbol),
                            Some(SymbolTypeFact::Local { mutable: true, .. })
                        ) {
                            retained.insert(*symbol);
                        } else {
                            pending.push(Value::Binding(*symbol));
                        }
                    }
                }
                HirExpr::Path(path) => {
                    if let ResolveResult::Resolved(symbol) = path.resolution {
                        pending.push(Value::Binding(symbol));
                    }
                }
                HirExpr::Tuple { elems, .. }
                | HirExpr::Array { elems, .. }
                | HirExpr::List { elems, .. }
                | HirExpr::Set { elems, .. } => {
                    pending.extend(elems.iter().map(|expr| Value::Expr(*expr)));
                }
                HirExpr::ListCons { head, tail, .. } => {
                    pending.extend([Value::Expr(*head), Value::Expr(*tail)])
                }
                HirExpr::Map { entries, .. } => {
                    for entry in entries {
                        pending.extend([Value::Expr(entry.key), Value::Expr(entry.value)]);
                    }
                }
                HirExpr::Record(record) => {
                    for field in &record.fields {
                        match field {
                            HirFieldInit::Named { value, .. } => pending.push(Value::Expr(*value)),
                            HirFieldInit::Shorthand {
                                resolution: ResolveResult::Resolved(symbol),
                                ..
                            } => pending.push(Value::Binding(*symbol)),
                            _ => (),
                        }
                    }
                }
                HirExpr::Block(block) => {
                    pending.extend(hir.blocks[*block].final_expr.map(Value::Expr))
                }
                _ => (),
            },
            _ => (),
        }
    }
    retained
}
