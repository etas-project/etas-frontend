use etas_hir::{
    HirAnnotationArg, HirExpr, HirExprId, HirItemId, HirRevision, ResolveResult, SymbolDef,
};

pub(super) fn called_item_has_marker(
    revision: &HirRevision,
    callee: HirExprId,
    marker: &str,
) -> bool {
    let HirExpr::Path(path) = &revision.exprs[callee] else {
        return false;
    };
    let ResolveResult::Resolved(symbol) = path.resolution else {
        return false;
    };
    let Some(symbol) = revision.symbols.get(symbol) else {
        return false;
    };
    let SymbolDef::Item { item } = symbol.def else {
        return false;
    };
    item_has_marker(revision, item, marker)
}

pub(super) fn item_has_marker(revision: &HirRevision, item: HirItemId, marker: &str) -> bool {
    revision
        .item_annotations
        .get(&item)
        .into_iter()
        .flatten()
        .filter(|annotation| {
            annotation
                .path
                .segments
                .last()
                .is_some_and(|segment| segment.name == "optimization")
        })
        .flat_map(|annotation| &annotation.args)
        .flat_map(|argument| {
            let value = match argument {
                HirAnnotationArg::Positional { value, .. }
                | HirAnnotationArg::Named { value, .. } => *value,
            };
            match &revision.exprs[value] {
                HirExpr::Array { elems, .. } | HirExpr::List { elems, .. } => elems.clone(),
                _ => vec![value],
            }
        })
        .any(|entry| {
            let HirExpr::Path(path) = &revision.exprs[entry] else {
                return false;
            };
            let ResolveResult::Resolved(symbol) = path.resolution else {
                return false;
            };
            revision
                .symbols
                .get(symbol)
                .is_some_and(|symbol| symbol.name == marker)
        })
}
