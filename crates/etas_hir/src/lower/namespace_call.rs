use etas_syntax::ast;

use super::program::LowerCtx;
use crate::{HirExpr, HirExprId, PathSegment, ResolveResult, SymbolDef};

impl LowerCtx {
    pub(super) fn lower_std_namespace_callee(
        &mut self,
        receiver: HirExprId,
        member: &ast::Name,
    ) -> bool {
        let HirExpr::Path(path) = &self.hir.exprs[receiver] else {
            return false;
        };
        let mut qualified = match &path.resolution {
            ResolveResult::Resolved(symbol) => {
                let Some(symbol) = self.symbols.get(*symbol) else {
                    return false;
                };
                let SymbolDef::ImportAlias { path, .. } = &symbol.def else {
                    return false;
                };
                path.clone()
            }
            ResolveResult::PartiallyResolved(partial) => {
                if let Some(prefix) = partial.resolved_prefix {
                    let Some(symbol) = self.symbols.get(prefix) else {
                        return false;
                    };
                    let SymbolDef::ImportAlias { path, .. } = &symbol.def else {
                        return false;
                    };
                    let mut qualified = path.clone();
                    qualified.extend(partial.remaining.iter().cloned());
                    qualified
                } else {
                    path.segments
                        .iter()
                        .map(|segment| segment.name.clone())
                        .collect()
                }
            }
            ResolveResult::Unresolved | ResolveResult::Ambiguous(_) => return false,
        };
        // Associated enum constructors are static namespace members, not methods.
        let enum_constructor = self
            .std_registry
            .lookup_qualified(&qualified)
            .and_then(|owner| self.std_registry.enum_constructor(owner.id, &member.text))
            .is_some();
        if !self
            .std_registry
            .modules()
            .any(|module| module.path == qualified)
            && !enum_constructor
        {
            return false;
        }
        qualified.push(member.text.clone());
        let mut path = path.clone();
        path.span.range.end = member.span.range.end;
        let Some(symbol) = self.resolve_std_qualified_path(&qualified, path.span) else {
            return false;
        };
        path.syntax_path.segments.push(member.clone());
        path.syntax_path.span = path.span;
        path.segments.push(PathSegment {
            name: member.text.clone(),
            span: member.span,
        });
        path.resolution = ResolveResult::Resolved(symbol);
        self.source_map.map_expr(receiver, path.span);
        // Reuse the receiver slot so the namespace path does not become an
        // orphan expression when the call is normalized.
        *self
            .hir
            .exprs
            .get_mut(receiver)
            .expect("lowered namespace receiver exists") = HirExpr::Path(path);
        true
    }
}
