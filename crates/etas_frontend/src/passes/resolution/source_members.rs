use etas_hir::{
    HirExpr, HirItem, HirItemId, HirPat, HirProgram, PathSegment, ResolveResult, ResolvedPath,
    SymbolId,
};

/// Install associated members only after the import resolver has checked visibility.
pub(super) fn bind_members(hir: &mut HirProgram, alias: SymbolId, item: HirItemId) {
    let Some(HirItem::Enum(decl)) = hir.items.get(item) else {
        return;
    };
    for variant in &decl.variants {
        if let Some(symbol) = hir.symbols.get(variant.symbol) {
            hir.symbols
                .insert_member(alias, symbol.name.clone(), variant.symbol);
        }
    }
}

fn resolve_path(
    path: &mut ResolvedPath,
    symbols: &etas_hir::SymbolTable,
    scopes: &etas_hir::ScopeTree,
) -> bool {
    if matches!(path.resolution, ResolveResult::Unresolved)
        && let [owner, name] = path.segments.as_slice()
        && let Some(scope) = scopes.iter().find(|scope| {
            matches!(scope.owner, etas_hir::ScopeOwner::Module(_))
                && scope.span.source == path.span.source
                && scope.parent.is_some_and(|parent| {
                    scopes
                        .get(parent)
                        .is_some_and(|parent| parent.owner == scope.owner)
                })
        })
        && let ResolveResult::Resolved(owner) = scopes.lookup(scope.id, &owner.name)
    {
        let resolution = symbols.resolve_member(owner, &name.name);
        if !matches!(resolution, ResolveResult::Unresolved) {
            path.resolution = resolution;
            return true;
        }
    }
    if let ResolveResult::PartiallyResolved(partial) = &path.resolution
        && let Some(owner) = partial.resolved_prefix
        && let [name] = partial.remaining.as_slice()
    {
        let resolution = symbols.resolve_member(owner, name);
        if !matches!(resolution, ResolveResult::Unresolved) {
            path.resolution = resolution;
            return true;
        }
    }
    false
}

pub(super) fn resolve_members(hir: &mut HirProgram) -> Vec<etas_core::Span> {
    let mut resolved = Vec::new();
    let exprs = hir
        .exprs
        .iter()
        .map(|(id, expr)| (id, expr.clone()))
        .collect::<Vec<_>>();
    for (id, mut expr) in exprs {
        match &mut expr {
            HirExpr::Path(path) => {
                if resolve_path(path, &hir.symbols, &hir.scopes) {
                    resolved.push(path.span);
                }
            }
            HirExpr::Record(record) => {
                if let Some(path) = &mut record.path
                    && resolve_path(path, &hir.symbols, &hir.scopes)
                {
                    resolved.push(path.span);
                }
            }
            HirExpr::MethodCall {
                receiver,
                method,
                generic_args,
                args,
                span,
            } => {
                if let Some(HirExpr::Path(path)) = hir.exprs.get(*receiver)
                    && let ResolveResult::Resolved(owner) = path.resolution
                {
                    let resolution = hir.symbols.resolve_member(owner, method);
                    if !matches!(resolution, ResolveResult::Unresolved) {
                        let mut path = path.clone();
                        path.segments.push(PathSegment {
                            name: method.clone(),
                            span: *span,
                        });
                        path.resolution = resolution;
                        if let Some(slot) = hir.exprs.get_mut(*receiver) {
                            *slot = HirExpr::Path(path);
                        }
                        expr = HirExpr::Call {
                            callee: *receiver,
                            generic_args: generic_args.clone(),
                            args: args.clone(),
                            span: *span,
                        };
                    }
                }
            }
            _ => {}
        }
        if let Some(slot) = hir.exprs.get_mut(id) {
            *slot = expr;
        }
    }
    let patterns = hir
        .pats
        .iter()
        .map(|(id, pat)| (id, pat.clone()))
        .collect::<Vec<_>>();
    for (id, mut pat) in patterns {
        match &mut pat {
            HirPat::Variant { path, .. }
            | HirPat::Record {
                path: Some(path), ..
            } => {
                if resolve_path(path, &hir.symbols, &hir.scopes) {
                    resolved.push(path.span);
                }
            }
            _ => {}
        }
        if let Some(slot) = hir.pats.get_mut(id) {
            *slot = pat;
        }
    }
    resolved
}
