use std::collections::{BTreeSet, HashMap};

use etas_hir::{HirExpr, HirExprId, HirItemId, ResolveResult, SymbolDef};
use etas_types::{TypeOutput, method::MethodTarget};

pub(super) fn external_signature<'a>(
    hir: &etas_hir::HirProgram,
    types: &'a TypeOutput,
    expr: HirExprId,
) -> Option<&'a etas_types::CallableSignature> {
    let path = match hir.exprs.get(expr)? {
        HirExpr::Call { callee, .. } => match hir.exprs.get(*callee)? {
            HirExpr::Path(path) => path,
            _ => return None,
        },
        HirExpr::Path(path) => path,
        _ => return None,
    };
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return None;
    };
    let symbol = hir.canonical_source_symbol(symbol)?;
    if !matches!(hir.symbols.get(symbol)?.def, SymbolDef::ImportAlias { .. }) {
        return None;
    }
    match types.facts.symbol_types.get(&symbol)? {
        etas_types::SymbolTypeFact::Flow { signature }
        | etas_types::SymbolTypeFact::Tool { signature } => Some(signature),
        etas_types::SymbolTypeFact::Agent { signature } => signature.callable(),
        _ => None,
    }
}

pub(crate) fn direct_targets(
    hir: &etas_hir::HirProgram,
    types: &TypeOutput,
) -> Result<HashMap<HirExprId, BTreeSet<HirItemId>>, String> {
    let mut targets = HashMap::<_, BTreeSet<_>>::new();
    for (expr, method) in types.facts.checked_method_calls.iter().chain(
        types
            .facts
            .bound_method_values
            .iter()
            .map(|(expr, value)| (expr, &value.method)),
    ) {
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
