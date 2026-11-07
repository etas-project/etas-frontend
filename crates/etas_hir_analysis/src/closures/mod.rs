use etas_hir::{
    HirExpr, HirExprId, HirFieldInit, HirNodeRef, HirProgram, HirTreeView, ResolveResult,
    SymbolDef, SymbolId, SymbolKind,
};
use std::collections::{BTreeSet, HashMap, HashSet};
mod scopes;
pub use scopes::{ClosureScope, ClosureScopes};

/// Runtime locals use the same classification in capture analysis and frame planning.
pub fn is_runtime_local(kind: SymbolKind, def: &SymbolDef) -> bool {
    matches!(kind, SymbolKind::Param | SymbolKind::Local)
        || matches!(def, SymbolDef::PatternBinding { .. })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClosureCaptureLayout {
    pub captures: Vec<SymbolId>,
    pub locals: Vec<SymbolId>,
}

/// Captures are sorted symbol identities, including values forwarded to nested closures.
pub fn analyze_closures(
    view: &HirTreeView<'_>,
) -> Result<HashMap<HirExprId, ClosureCaptureLayout>, String> {
    let hir: &HirProgram = view.program();
    let local_symbols: HashSet<_> = hir
        .symbols
        .iter()
        .filter(|symbol| is_runtime_local(symbol.kind, &symbol.def))
        .map(|symbol| symbol.id)
        .collect();
    let mut captures = HashMap::<_, BTreeSet<SymbolId>>::new();
    let mut locals = HashMap::<_, BTreeSet<SymbolId>>::new();
    for (expr, data) in hir.exprs.iter() {
        if matches!(data, HirExpr::Lambda { .. }) {
            captures.insert(expr, BTreeSet::new());
            locals.insert(expr, BTreeSet::new());
        }
    }
    if captures.is_empty() {
        return Ok(HashMap::new());
    }
    let scope_index = ClosureScopes::build(&hir.scopes)?;
    let mut declarations = HashMap::new();
    for scope in hir.scopes.iter() {
        for symbol in scope
            .symbols
            .iter()
            .copied()
            .filter(|symbol| local_symbols.contains(symbol))
        {
            if let Some(previous) = declarations.insert(symbol, scope.id)
                && previous != scope.id
            {
                return Err(format!(
                    "local symbol {symbol:?} has multiple declaring scopes"
                ));
            }
            if let Some(closure) = scope_index.nearest(scope.id)? {
                locals
                    .get_mut(&closure.expr)
                    .ok_or("lambda scope has no expression")?
                    .insert(symbol);
            }
        }
    }

    // Each resolved use contributes to every intervening closure, including an
    // outer closure that only forwards the binding to a nested closure.
    for (expr, data) in hir.exprs.iter() {
        let mut record_use = |resolution: &ResolveResult| -> Result<(), String> {
            let symbol = match resolution {
                ResolveResult::Resolved(symbol) => *symbol,
                ResolveResult::PartiallyResolved(partial) => match partial.resolved_prefix {
                    Some(symbol) => symbol,
                    None => return Ok(()),
                },
                _ => return Ok(()),
            };
            if !local_symbols.contains(&symbol) {
                return Ok(());
            }
            let declaration = *declarations
                .get(&symbol)
                .ok_or_else(|| format!("local symbol {symbol:?} has no declaring scope"))?;
            let scope = view
                .enclosing_scope(HirNodeRef::Expr(expr))
                .ok_or("local use has no enclosing scope")?;
            if !scope_index.contains(declaration, scope)? {
                return Err(format!(
                    "local symbol {symbol:?} is outside the lexical scope of {expr:?}"
                ));
            }
            let mut nearest = scope_index.nearest(scope)?;
            while let Some(closure) = nearest {
                if closure.scope == declaration
                    || !scope_index.contains(declaration, closure.scope)?
                {
                    break;
                }
                captures
                    .get_mut(&closure.expr)
                    .ok_or("lambda scope has no expression")?
                    .insert(symbol);
                nearest = scope_index.outer(closure)?;
            }
            Ok(())
        };
        match data {
            HirExpr::Path(path) => record_use(&path.resolution)?,
            HirExpr::Reflect(reflection) => record_use(&reflection.target.root_path().resolution)?,
            HirExpr::Record(record) => {
                for field in &record.fields {
                    if let HirFieldInit::Shorthand { resolution, .. } = field {
                        record_use(resolution)?;
                    }
                }
            }
            _ => {}
        }
    }
    captures
        .into_iter()
        .map(|(expr, captures)| {
            let locals = locals.remove(&expr).ok_or("missing closure locals")?;
            Ok((
                expr,
                ClosureCaptureLayout {
                    captures: captures.into_iter().collect(),
                    locals: locals.into_iter().collect(),
                },
            ))
        })
        .collect()
}
