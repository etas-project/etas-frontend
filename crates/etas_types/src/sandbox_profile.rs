//! Static identity of a sandbox profile, shared by selectors and value provenance.
use etas_hir::{
    HirExpr, HirProgram, PartialResolutionReason, ResolveResult, ResolvedPath, SymbolDef, SymbolId,
};
use etas_std::StdRegistry;

pub const TYPE_NAME: &str = "std.host.sandbox.SandboxProfile";

pub(crate) fn is_selector_kind(kind: &crate::EffectActionArgKind) -> bool {
    matches!(kind, crate::EffectActionArgKind::StaticResourcePath { ty }
        if matches!(ty.as_str(), "SandboxProfile" | TYPE_NAME))
}

/// Distinct record initializers introduce identities. A top-level name whose
/// entire initializer is another name preserves that declaration's identity.
pub fn origin_symbol(hir: &HirProgram, mut symbol: SymbolId) -> Option<SymbolId> {
    let mut visited = std::collections::HashSet::new();
    loop {
        symbol = hir.canonical_source_symbol(symbol)?;
        match &hir.symbols.get(symbol)?.def {
            SymbolDef::TopLevelLet { initializer, .. } => {
                if let Some(HirExpr::Path(path)) = hir.exprs.get(*initializer)
                    && let ResolveResult::Resolved(target) = path.resolution
                {
                    if !visited.insert(symbol) {
                        return None;
                    }
                    symbol = target;
                } else {
                    return Some(symbol);
                }
            }
            SymbolDef::ImportAlias { .. } => return Some(symbol),
            _ => return None,
        }
    }
}

fn registry_path(registry: &StdRegistry, path: &[String]) -> Option<Vec<String>> {
    let declaration = registry.lookup_qualified(path)?;
    let key = registry.declaration_key(declaration.id)?;
    Some(
        key.module()
            .iter()
            .chain(key.declaration())
            .cloned()
            .collect(),
    )
}

fn qualified_path(
    hir: &HirProgram,
    registry: &StdRegistry,
    path: &ResolvedPath,
) -> Option<Vec<String>> {
    let ResolveResult::PartiallyResolved(partial) = &path.resolution else {
        return None;
    };
    if partial.reason != PartialResolutionReason::MemberRequiresTypeChecking {
        return None;
    }
    let prefix = partial.resolved_prefix?;
    let mut segments = match &hir.symbols.get(prefix)?.def {
        SymbolDef::ImportAlias { path, .. } => path.clone(),
        _ => hir.declaration_path(prefix)?,
    };
    segments.extend_from_slice(&partial.remaining);
    registry_path(registry, &segments)
}

pub fn declaration_path(
    hir: &HirProgram,
    registry: &StdRegistry,
    symbol: SymbolId,
) -> Option<Vec<String>> {
    let origin = origin_symbol(hir, symbol)?;
    match &hir.symbols.get(origin)?.def {
        SymbolDef::ImportAlias { path, .. } => {
            Some(registry_path(registry, path).unwrap_or_else(|| path.clone()))
        }
        SymbolDef::TopLevelLet { initializer, .. } => {
            if let Some(HirExpr::Path(path)) = hir.exprs.get(*initializer) {
                return qualified_path(hir, registry, path);
            }
            hir.declaration_path(origin)
        }
        _ => None,
    }
}

pub fn selector_path(
    hir: &HirProgram,
    registry: &StdRegistry,
    path: &ResolvedPath,
) -> Option<Vec<String>> {
    if let ResolveResult::Resolved(symbol) = path.resolution {
        declaration_path(hir, registry, symbol)
    } else {
        qualified_path(hir, registry, path)
    }
}
