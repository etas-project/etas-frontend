use etas_hir::{
    HirExpr, HirExprId, HirItem, HirOwner, HirReceiverMode, HirRevision, ResolveResult, SymbolDef,
    SymbolId,
};

use crate::{SymbolTypeFact, TypeFacts};

/// Resolve the checked local binding, rather than treating a field, temporary,
/// ordinary parameter, or captured receiver as update access.
pub fn updating_receiver_symbol(
    hir: &HirRevision,
    facts: &TypeFacts,
    call: HirExprId,
) -> Result<SymbolId, String> {
    let receiver = match hir.exprs.get(call) {
        Some(HirExpr::MethodCall { receiver, .. } | HirExpr::SpecMethodCall { receiver, .. }) => {
            *receiver
        }
        _ => return Err("updating receiver requires a method call".into()),
    };
    let Some(HirExpr::Path(path)) = hir.exprs.get(receiver) else {
        return Err("updating receiver requires a direct local var binding".into());
    };
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return Err("updating receiver requires a resolved local var binding".into());
    };
    let view = hir.view();
    let index = view.index();
    let owner = |mut owner: HirOwner| {
        loop {
            let parent = match owner {
                HirOwner::Expr(expr)
                    if !matches!(hir.exprs.get(expr), Some(HirExpr::Lambda { .. })) =>
                {
                    index.expr_owner.get(&expr)
                }
                HirOwner::Body(body) => index.body_owner.get(&body),
                HirOwner::Block(block) => index.block_owner.get(&block),
                HirOwner::Stmt(stmt) => index
                    .stmt_block
                    .get(&stmt)
                    .and_then(|block| index.block_owner.get(block)),
                _ => None,
            };
            let Some(parent) = parent else {
                break;
            };
            owner = *parent;
        }
        owner
    };
    let current = owner(
        *index
            .expr_owner
            .get(&call)
            .ok_or("updating call has no checked owner")?,
    );
    let declaration = hir
        .symbols
        .get(symbol)
        .ok_or("updating receiver has no declaration")?;
    let eligible = match &declaration.def {
        SymbolDef::Local { binding, .. } => {
            matches!(
                facts.symbol_types.get(&symbol),
                Some(SymbolTypeFact::Local { mutable: true, .. })
            ) && index
                .stmt_block
                .get(binding)
                .and_then(|block| index.block_owner.get(block))
                .is_some_and(|binding_owner| owner(*binding_owner) == current)
        }
        SymbolDef::Param {
            owner: item,
            param_index: 0,
            ..
        } => {
            current == HirOwner::Item(*item)
                && matches!(hir.items.get(*item), Some(HirItem::Flow(flow))
                    if flow.receiver_mode == HirReceiverMode::Updating && flow.params.first() == Some(&symbol))
        }
        _ => false,
    };
    if !eligible {
        return Err(
            "updating receiver requires a direct local var or the current method's var self".into(),
        );
    }
    Ok(symbol)
}
