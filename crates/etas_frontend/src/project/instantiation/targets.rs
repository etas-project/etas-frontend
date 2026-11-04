use std::collections::{BTreeSet, HashMap};

use etas_hir::{HirExpr, HirExprId, HirItemId, ResolveResult, SymbolDef};
use etas_types::{TypeOutput, method::MethodTarget};

pub(crate) fn direct_targets(
    hir: &etas_hir::HirProgram,
    types: &TypeOutput,
) -> Result<HashMap<HirExprId, BTreeSet<HirItemId>>, String> {
    let mut targets = HashMap::<_, BTreeSet<_>>::new();
    for (expr, method) in &types.facts.checked_method_calls {
        if let MethodTarget::Source { callable, .. } | MethodTarget::Agent { callable, .. } =
            method.target
        {
            targets.entry(*expr).or_default().insert(callable);
        }
    }
    for (expr, fact) in &types.facts.generic_instantiations {
        if !fact.deferred_effect_row_obligations.is_empty() {
            continue;
        }
        let path = match hir.exprs.get(*expr) {
            Some(HirExpr::Call { callee, .. }) => match hir.exprs.get(*callee) {
                Some(HirExpr::Path(path)) => path,
                _ => continue,
            },
            Some(HirExpr::Path(path)) => path,
            _ => continue,
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            continue;
        };
        let symbol = hir
            .canonical_source_symbol(symbol)
            .ok_or("direct generic call has an invalid source binding")?;
        let Some(SymbolDef::Item { item }) = hir.symbols.get(symbol).map(|symbol| &symbol.def)
        else {
            continue;
        };
        if super::signature(&types.facts, *item).is_some() {
            targets.entry(*expr).or_default().insert(*item);
        }
    }
    Ok(targets)
}
