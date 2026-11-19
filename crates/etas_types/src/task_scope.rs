//! Scoped-callback promises are inferred from source uses, never from API names.
use std::collections::{HashMap, HashSet};

use crate::{
    ItemSignature, SymbolTypeFact, Type, TypeFacts,
    facts::{CheckedMethodCallFact, concurrency::ScopedCallbackFact},
    pipeline::context::TypePipelineContext,
};
use etas_hir::{
    HirArg, HirBodyKind, HirExpr, HirExprId, HirFlowBody, HirItem, HirOwner, HirProgram,
    ResolveResult, SymbolId,
};

/// A callback is scoped only if every use is a call inside a drained scope or a
/// transfer to another callable with this same checked promise. Cycles alone
/// establish no promise; inference grows from the actual scope-entry mechanism.
pub(crate) fn infer(ctx: &mut TypePipelineContext<'_>) {
    let hir = ctx.hir;
    let view = ctx
        .hir_view
        .get_or_insert_with(|| etas_hir::HirTreeView::new(hir));
    let store = ctx.interner.store();
    let facts = &mut ctx.signature_facts;
    for (_, item) in hir.items.iter() {
        if let HirItem::Flow(flow) = item
            && matches!(
                flow.body,
                HirFlowBody::Runtime {
                    role: etas_std::StdExecutionRole::ScopeEnter,
                    ..
                }
            )
        {
            facts.scoped_callbacks.insert(
                flow.symbol,
                ScopedCallbackFact {
                    parameters: vec![1],
                    parameter_names: names(hir, &flow.params),
                },
            );
        }
    }
    let mut uses: HashMap<SymbolId, Vec<HirExprId>> = HashMap::new();
    for (expr, value) in hir.exprs.iter() {
        if let HirExpr::Path(path) = value
            && let ResolveResult::Resolved(symbol) = path.resolution
        {
            uses.entry(symbol).or_default().push(expr);
        }
    }
    loop {
        let scoped_calls = hir
            .exprs
            .iter()
            .filter_map(|(call, _)| {
                let args = arguments(hir, facts, call, facts.checked_method_calls.get(&call));
                (!args.is_empty()).then_some((call, args))
            })
            .collect::<Vec<_>>();
        let roots = scoped_calls
            .iter()
            .flat_map(|(_, args)| args)
            .copied()
            .filter(|arg| matches!(hir.exprs.get(*arg), Some(HirExpr::Lambda { .. })))
            .collect::<HashSet<_>>();
        let forwarded = scoped_calls
            .iter()
            .flat_map(|(_, args)| args)
            .copied()
            .filter(|arg| use_domain(view.index(), *arg, &roots, true))
            .collect::<HashSet<_>>();
        let mut scoped_uses = forwarded;
        for (call, value) in hir.exprs.iter() {
            if let HirExpr::Call { callee, .. } = value
                && inside_scope(view.index(), call, &roots)
            {
                scoped_uses.insert(*callee);
            }
        }
        let mut changed = false;
        for (item, declaration) in hir.items.iter() {
            let HirItem::Flow(flow) = declaration else {
                continue;
            };
            if !matches!(flow.body, HirFlowBody::Block(_) | HirFlowBody::Expr { .. }) {
                continue;
            }
            let Some(ItemSignature::Flow(signature)) = facts.item_signatures.get(&item) else {
                continue;
            };
            let parameters = flow
                .params
                .iter()
                .zip(&signature.params)
                .enumerate()
                .filter_map(|(slot, (symbol, ty))| {
                    let occurrences = uses.get(symbol)?;
                    (matches!(store.get(*ty), Some(Type::Function(_)))
                        && !occurrences.is_empty()
                        && occurrences.iter().all(|expr| scoped_uses.contains(expr)))
                    .then_some(slot as u32)
                })
                .collect::<Vec<_>>();
            if parameters.is_empty() {
                continue;
            }
            if facts
                .scoped_callbacks
                .get(&flow.symbol)
                .is_none_or(|known| known.parameters != parameters)
            {
                facts.scoped_callbacks.insert(
                    flow.symbol,
                    ScopedCallbackFact {
                        parameters,
                        parameter_names: names(hir, &flow.params),
                    },
                );
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// Actual argument expressions at the promised ABI slots. Checked method calls
/// already retain named-argument binding; source signature inference only needs
/// unambiguous associated calls such as the actual private Scope.enter method.
pub fn arguments(
    hir: &HirProgram,
    facts: &TypeFacts,
    call: HirExprId,
    checked: Option<&CheckedMethodCallFact>,
) -> Vec<HirExprId> {
    if let Some(checked) = checked {
        let slots = match &checked.target {
            crate::method::MethodTarget::Source { symbol, .. } => facts
                .scoped_callbacks
                .get(symbol)
                .map(|fact| &fact.parameters),
            crate::method::MethodTarget::External { identity } => {
                facts.external_scoped_methods.get(identity)
            }
            _ => None,
        };
        return slots
            .into_iter()
            .flatten()
            .filter_map(|slot| crate::method::argument_expression(hir, call, checked, *slot))
            .collect();
    }
    match hir.exprs.get(call) {
        Some(HirExpr::Call { callee, args, .. }) => {
            let Some(symbol) = crate::source_execution::callable_symbol(hir, *callee) else {
                return Vec::new();
            };
            let Some(contract) = facts.scoped_callbacks.get(&symbol) else {
                return Vec::new();
            };
            bind_arguments(args, &contract.parameter_names)
                .into_iter()
                .filter_map(|(slot, expr)| contract.parameters.contains(&slot).then_some(expr))
                .collect()
        }
        Some(HirExpr::MethodCall {
            receiver,
            method,
            args,
            ..
        }) => {
            let Some(selected) = associated_method(hir, facts, *receiver, method) else {
                return Vec::new();
            };
            let slots = match &selected.target {
                crate::method::MethodTarget::Source { symbol, .. } => facts
                    .scoped_callbacks
                    .get(symbol)
                    .map(|contract| &contract.parameters),
                crate::method::MethodTarget::External { identity } => {
                    facts.external_scoped_methods.get(identity)
                }
                _ => None,
            };
            let Some(slots) = slots else {
                return Vec::new();
            };
            let names = selected
                .parameter_names
                .iter()
                .map(|name| name.clone().unwrap_or_default())
                .collect::<Vec<_>>();
            bind_arguments(args, &names)
                .into_iter()
                .filter_map(|(slot, expr)| slots.contains(&slot).then_some(expr))
                .collect()
        }
        _ => Vec::new(),
    }
}

fn associated_method<'a>(
    hir: &HirProgram,
    facts: &'a TypeFacts,
    receiver: HirExprId,
    name: &str,
) -> Option<&'a crate::method::MethodSignatureFact> {
    let HirExpr::Path(path) = hir.exprs.get(receiver)? else {
        return None;
    };
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return None;
    };
    let symbol = hir.canonical_source_symbol(symbol)?;
    let owner = match facts.symbol_types.get(&symbol)? {
        SymbolTypeFact::NominalType { constructor, .. } | SymbolTypeFact::Type { constructor } => {
            crate::TypeId(constructor.0)
        }
        SymbolTypeFact::TypeAlias { target, .. } => *target,
        _ => return None,
    };
    let mut candidates = facts.methods.entries().filter(|candidate| {
        candidate.name == name
            && candidate.receiver_type == owner
            && candidate.receiver == crate::method::MethodReceiver::Associated
            && matches!(candidate.owner, crate::method::MethodOwner::Inherent(_))
    });
    let candidate = candidates.next()?;
    if candidates.next().is_some() {
        return None;
    }
    Some(candidate)
}

fn bind_arguments(args: &[HirArg], names: &[String]) -> Vec<(u32, HirExprId)> {
    let mut bound = HashSet::new();
    let mut result = Vec::new();
    for arg in args {
        let (slot, expr) = match arg {
            HirArg::Named { name, value, .. } => {
                (names.iter().position(|parameter| parameter == name), *value)
            }
            HirArg::Positional(expr) => {
                ((0..names.len()).find(|slot| !bound.contains(slot)), *expr)
            }
        };
        if let Some(slot) = slot {
            bound.insert(slot);
            result.push((slot as u32, expr));
        }
    }
    result
}

fn names(hir: &HirProgram, parameters: &[SymbolId]) -> Vec<String> {
    parameters
        .iter()
        .map(|symbol| {
            hir.symbols
                .get(*symbol)
                .expect("declared parameter")
                .name
                .clone()
        })
        .collect()
}

fn inside_scope(
    index: &etas_hir::HirTreeIndex,
    expr: HirExprId,
    roots: &HashSet<HirExprId>,
) -> bool {
    use_domain(index, expr, roots, false)
}

fn use_domain(
    index: &etas_hir::HirTreeIndex,
    expr: HirExprId,
    roots: &HashSet<HirExprId>,
    allow_item: bool,
) -> bool {
    let mut owner = index.expr_owner.get(&expr).copied();
    while let Some(current) = owner {
        owner = match current {
            HirOwner::Expr(expr) => index.expr_owner.get(&expr).copied(),
            HirOwner::Block(block) => index.block_owner.get(&block).copied(),
            HirOwner::Stmt(stmt) => index
                .stmt_block
                .get(&stmt)
                .and_then(|block| index.block_owner.get(block))
                .copied(),
            HirOwner::Body(body) => match body.kind {
                HirBodyKind::Lambda(lambda) => return roots.contains(&lambda),
                HirBodyKind::Flow | HirBodyKind::ImplFlow(_) => return allow_item,
                HirBodyKind::MatchArm { expr, .. } | HirBodyKind::StageLimit { expr, .. } => {
                    index.expr_owner.get(&expr).copied()
                }
                _ => return false,
            },
            _ => return false,
        };
    }
    false
}
