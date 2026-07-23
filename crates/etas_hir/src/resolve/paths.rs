use etas_core::Span;
use etas_std::{StdDecl, StdRegistry};
use etas_syntax::ast;

use crate::{HirDiagnostics, HirEffectRef, ScopeId, ScopeTree, SymbolId, SymbolKind, SymbolTable};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveResult {
    Resolved(SymbolId),
    PartiallyResolved(PartialResolution),
    Unresolved,
    Ambiguous(Vec<SymbolId>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartialResolution {
    pub resolved_prefix: Option<SymbolId>,
    pub resolved_segments: u32,
    pub remaining: Vec<String>,
    pub reason: PartialResolutionReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartialResolutionReason {
    MemberRequiresTypeChecking,
    ModuleMemberMissing,
    UnsupportedPathShape,
    PackageResolverRequired,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedPath {
    pub syntax_path: ast::Path,
    pub segments: Vec<PathSegment>,
    pub resolution: ResolveResult,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathSegment {
    pub name: String,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedSymbol {
    pub symbol: SymbolId,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedActionRef {
    pub effect: HirEffectRef,
    pub action: String,
    pub action_symbol: ResolveResult,
    pub span: Span,
}

pub fn path_text(path: &ast::Path) -> String {
    path.segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

pub fn path_segments(path: &ast::Path) -> Vec<PathSegment> {
    path.segments
        .iter()
        .map(|segment| PathSegment {
            name: segment.text.clone(),
            span: segment.span,
        })
        .collect()
}

pub fn unresolved_path_from_segments(names: &[&str], span: Span) -> ResolvedPath {
    let syntax_path = ast::Path {
        segments: names
            .iter()
            .map(|name| ast::Name {
                text: (*name).to_owned(),
                span,
            })
            .collect(),
        span,
    };
    ResolvedPath {
        segments: names
            .iter()
            .map(|name| PathSegment {
                name: (*name).to_owned(),
                span,
            })
            .collect(),
        syntax_path,
        resolution: ResolveResult::Unresolved,
        span,
    }
}

pub fn resolve_path(
    path: &ast::Path,
    scope: ScopeId,
    scopes: &ScopeTree,
    symbols: &SymbolTable,
    diagnostics: &mut HirDiagnostics,
    report: bool,
) -> ResolvedPath {
    let std_registry = etas_std::standard_registry();
    resolve_path_with_std_registry(
        path,
        scope,
        scopes,
        symbols,
        diagnostics,
        report,
        &std_registry,
    )
}

pub fn resolve_path_with_std_registry(
    path: &ast::Path,
    scope: ScopeId,
    scopes: &ScopeTree,
    symbols: &SymbolTable,
    diagnostics: &mut HirDiagnostics,
    report: bool,
    std_registry: &StdRegistry,
) -> ResolvedPath {
    let full_path = path_text(path);
    let resolution = if path.segments.len() > 1 {
        match resolve_name(&full_path, path.span, scope, scopes, diagnostics, false) {
            ResolveResult::Unresolved => resolve_qualified_prefix(
                path,
                scope,
                scopes,
                symbols,
                diagnostics,
                report,
                std_registry,
            ),
            ResolveResult::Ambiguous(symbols) => {
                if report {
                    diagnostics.ambiguous_name(&full_path, path.span);
                }
                ResolveResult::Ambiguous(symbols)
            }
            result => result,
        }
    } else {
        let name = path
            .segments
            .first()
            .map(|segment| segment.text.as_str())
            .unwrap_or_default();
        resolve_name(name, path.span, scope, scopes, diagnostics, report)
    };
    ResolvedPath {
        syntax_path: path.clone(),
        segments: path_segments(path),
        resolution,
        span: path.span,
    }
}

fn resolve_qualified_prefix(
    path: &ast::Path,
    scope: ScopeId,
    scopes: &ScopeTree,
    symbols: &SymbolTable,
    diagnostics: &mut HirDiagnostics,
    report: bool,
    std_registry: &StdRegistry,
) -> ResolveResult {
    let full_path = path_text(path);
    for prefix_len in (1..path.segments.len()).rev() {
        let prefix = path.segments[..prefix_len]
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(".");
        match resolve_name(&prefix, path.span, scope, scopes, diagnostics, false) {
            ResolveResult::Resolved(symbol) => {
                let remaining = path.segments[prefix_len..]
                    .iter()
                    .map(|segment| segment.text.clone())
                    .collect::<Vec<_>>();
                return ResolveResult::PartiallyResolved(PartialResolution {
                    resolved_prefix: Some(symbol),
                    resolved_segments: prefix_len.min(u32::MAX as usize) as u32,
                    remaining,
                    reason: partial_reason_for_prefix(symbol, symbols, std_registry),
                });
            }
            ResolveResult::Ambiguous(symbols) => {
                if report {
                    diagnostics.ambiguous_name(&prefix, path.span);
                }
                return ResolveResult::Ambiguous(symbols);
            }
            ResolveResult::PartiallyResolved(partial) => {
                return ResolveResult::PartiallyResolved(partial);
            }
            ResolveResult::Unresolved => {}
        }
    }

    if package_resolver_required(path) {
        ResolveResult::PartiallyResolved(PartialResolution {
            resolved_prefix: None,
            resolved_segments: 0,
            remaining: path
                .segments
                .iter()
                .map(|segment| segment.text.clone())
                .collect(),
            reason: PartialResolutionReason::PackageResolverRequired,
        })
    } else if unsupported_path_shape(path) {
        ResolveResult::PartiallyResolved(PartialResolution {
            resolved_prefix: None,
            resolved_segments: 0,
            remaining: path
                .segments
                .iter()
                .map(|segment| segment.text.clone())
                .collect(),
            reason: PartialResolutionReason::UnsupportedPathShape,
        })
    } else {
        if report && !full_path.is_empty() {
            diagnostics.unresolved_name(&full_path, path.span);
        }
        ResolveResult::Unresolved
    }
}

pub(crate) fn partial_reason_for_prefix(
    symbol: SymbolId,
    symbols: &SymbolTable,
    std_registry: &StdRegistry,
) -> PartialResolutionReason {
    match symbols.get(symbol) {
        Some(symbol) if symbol.kind == SymbolKind::Module => {
            PartialResolutionReason::ModuleMemberMissing
        }
        Some(symbol) if import_alias_points_to_std_type(&symbol.def, std_registry) => {
            PartialResolutionReason::MemberRequiresTypeChecking
        }
        Some(symbol) if matches!(symbol.def, crate::SymbolDef::ImportAlias { .. }) => {
            PartialResolutionReason::ModuleMemberMissing
        }
        Some(_) | None => PartialResolutionReason::MemberRequiresTypeChecking,
    }
}

fn import_alias_points_to_std_type(def: &crate::SymbolDef, std_registry: &StdRegistry) -> bool {
    let crate::SymbolDef::ImportAlias { path, .. } = def else {
        return false;
    };
    let Some(first) = path.first() else {
        return false;
    };
    if first != "std" {
        return false;
    }
    std_registry
        .lookup_qualified(path)
        .is_some_and(|symbol| matches!(symbol.decl, StdDecl::Type(_)))
}

pub(crate) fn package_resolver_required(path: &ast::Path) -> bool {
    let Some(first) = path.segments.first() else {
        return false;
    };
    matches!(
        first.text.as_str(),
        "std" | "host" | "runtime" | "provider" | "providers"
    )
}

pub(crate) fn unsupported_path_shape(path: &ast::Path) -> bool {
    path.segments.len() > 3
}

pub fn resolve_name(
    name: &str,
    span: Span,
    scope: ScopeId,
    scopes: &ScopeTree,
    diagnostics: &mut HirDiagnostics,
    report: bool,
) -> ResolveResult {
    match scopes.lookup(scope, name) {
        ResolveResult::Resolved(symbol) => ResolveResult::Resolved(symbol),
        ResolveResult::PartiallyResolved(partial) => ResolveResult::PartiallyResolved(partial),
        ResolveResult::Ambiguous(symbols) => {
            if report && !name.is_empty() {
                diagnostics.ambiguous_name(name, span);
            }
            ResolveResult::Ambiguous(symbols)
        }
        ResolveResult::Unresolved => {
            if report && !name.is_empty() {
                diagnostics.unresolved_name(name, span);
            }
            ResolveResult::Unresolved
        }
    }
}
