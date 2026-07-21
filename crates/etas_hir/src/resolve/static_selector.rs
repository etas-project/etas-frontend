use crate::{
    HirProgram, PartialResolutionReason, ResolveResult, ResolvedPath, SymbolDef, SymbolId,
    TopLevelLetClassification,
};

pub fn resolved_static_type_selector_symbol<F>(
    path: &ResolvedPath,
    mut canonicalize: F,
) -> Option<SymbolId>
where
    F: FnMut(SymbolId) -> Option<SymbolId>,
{
    let symbol = match &path.resolution {
        ResolveResult::Resolved(symbol) => *symbol,
        ResolveResult::PartiallyResolved(partial)
            if partial.reason == PartialResolutionReason::MemberRequiresTypeChecking
                && partial.remaining.is_empty() =>
        {
            partial.resolved_prefix?
        }
        _ => return None,
    };
    Some(canonicalize(symbol).unwrap_or(symbol))
}

pub fn static_selector_path_segments<F>(
    hir: &HirProgram,
    path: &ResolvedPath,
    mut canonicalize: F,
) -> Option<Vec<String>>
where
    F: FnMut(SymbolId) -> Option<SymbolId>,
{
    let (symbol, remaining) = match &path.resolution {
        ResolveResult::Resolved(symbol) => (*symbol, Vec::new()),
        ResolveResult::PartiallyResolved(partial)
            if partial.reason == PartialResolutionReason::MemberRequiresTypeChecking =>
        {
            (partial.resolved_prefix?, partial.remaining.clone())
        }
        _ => return None,
    };
    let symbol = canonicalize(symbol).unwrap_or(symbol);
    let symbol = hir.symbols.get(symbol)?;
    if let SymbolDef::ImportAlias { path, .. } = &symbol.def {
        let mut path = path.clone();
        path.extend(remaining);
        return Some(path);
    }
    if !is_static_selector_symbol(&symbol.def) {
        return None;
    }
    let mut segments = hir
        .modules_arena
        .get(symbol.defining_module)?
        .name
        .as_ref()
        .map(|module_name| {
            module_name
                .segments
                .iter()
                .map(|segment| segment.name.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    segments.push(symbol.name.clone());
    segments.extend(remaining);
    Some(segments)
}

pub fn is_static_selector_symbol(def: &SymbolDef) -> bool {
    match def {
        SymbolDef::Item { .. } | SymbolDef::EnumVariant { .. } | SymbolDef::Synthetic { .. } => {
            true
        }
        SymbolDef::TopLevelLet { classification, .. } => matches!(
            classification,
            TopLevelLetClassification::Unknown
                | TopLevelLetClassification::Const
                | TopLevelLetClassification::ResourceHandle(_)
                | TopLevelLetClassification::Handler
        ),
        _ => false,
    }
}
