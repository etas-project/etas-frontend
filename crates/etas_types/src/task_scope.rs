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
                    role: etas_std::StdExecutionRole::ScopeEnter
                        | etas_std::StdExecutionRole::ResourceScope,
                    ..
                }
            )
        {
            let resource = matches!(
                flow.body,
                HirFlowBody::Runtime {
                    role: etas_std::StdExecutionRole::ResourceScope,
                    ..
                }
            );
            facts.scoped_callbacks.insert(
                flow.symbol,
                ScopedCallbackFact {
                    parameters: if resource {
                        flow.params
                            .iter()
                            .enumerate()
                            .filter_map(|(index, symbol)| {
                                let crate::SymbolTypeFact::Param { ty } =
                                    facts.symbol_types.get(symbol)?
                                else {
                                    return None;
                                };
                                matches!(store.get(*ty), Some(crate::Type::Function(_)))
                                    .then_some(index as u32)
                            })
                            .collect()
                    } else {
                        vec![1]
                    },
                    parameter_names: names(hir, &flow.params),
                    resource_boundaries: if resource { vec![0] } else { Vec::new() },
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
        // Only forwarding the callback itself carries the resource boundary
        // obligation. Invoking it inside another boundary's body may consume
        // its result there and does not make that result escape.
        let resource_forwarded = hir
            .exprs
            .iter()
            .flat_map(|(call, _)| {
                resource_arguments(hir, facts, call, facts.checked_method_calls.get(&call))
            })
            .collect::<HashSet<_>>();
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
            let resource_boundaries = parameters
                .iter()
                .copied()
                .filter(|slot| {
                    uses.get(&flow.params[*slot as usize])
                        .is_some_and(|occurrences| {
                            occurrences
                                .iter()
                                .all(|expr| resource_forwarded.contains(expr))
                        })
                })
                .collect::<Vec<_>>();
            if facts
                .scoped_callbacks
                .get(&flow.symbol)
                .is_none_or(|known| {
                    known.parameters != parameters
                        || known.resource_boundaries != resource_boundaries
                })
            {
                facts.scoped_callbacks.insert(
                    flow.symbol,
                    ScopedCallbackFact {
                        parameters,
                        parameter_names: names(hir, &flow.params),
                        resource_boundaries,
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
    arguments_for(hir, facts, call, checked, false)
}

/// Checked callback results crossing a fresh resource boundary. Unlike task
/// callbacks, these require an owner-relative resource escape proof.
pub fn resource_arguments(
    hir: &HirProgram,
    facts: &TypeFacts,
    call: HirExprId,
    checked: Option<&CheckedMethodCallFact>,
) -> Vec<HirExprId> {
    arguments_for(hir, facts, call, checked, true)
}

fn slots(contract: &ScopedCallbackFact, resource: bool) -> &[u32] {
    if resource {
        &contract.resource_boundaries
    } else {
        &contract.parameters
    }
}

fn arguments_for(
    hir: &HirProgram,
    facts: &TypeFacts,
    call: HirExprId,
    checked: Option<&CheckedMethodCallFact>,
    resource: bool,
) -> Vec<HirExprId> {
    let external = if resource {
        &facts.external_resource_boundaries
    } else {
        &facts.external_scoped_methods
    };
    if let Some(checked) = checked {
        let slots = match &checked.target {
            crate::method::MethodTarget::Source { symbol, .. } => facts
                .scoped_callbacks
                .get(symbol)
                .map(|fact| slots(fact, resource)),
            crate::method::MethodTarget::External { identity } => {
                external.get(identity).map(Vec::as_slice)
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
                .filter_map(|(slot, expr)| {
                    slots(contract, resource).contains(&slot).then_some(expr)
                })
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
                    .map(|contract| slots(contract, resource)),
                crate::method::MethodTarget::External { identity } => {
                    external.get(identity).map(Vec::as_slice)
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
                HirBodyKind::MatchArm { expr, .. } => index.expr_owner.get(&expr).copied(),
                _ => return false,
            },
            _ => return false,
        };
    }
    false
}
