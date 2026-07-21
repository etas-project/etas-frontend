//! Canonical lexical scope storage for name resolution.
//!
//! `ScopeTree` owns lexical scope records, parent links, and per-namespace symbol
//! bindings. It is the source of truth for lookup. Structural HIR traversal lives
//! in `crate::view`; `HirTreeIndex` may derive scope-owner and scope-child indexes
//! from this storage, but passes must not treat those indexes as a second scope
//! store.

use std::collections::HashMap;

use etas_core::Span;

use crate::{HirBlockId, HirExprId, HirItemId, HirModuleId, ResolveResult, ScopeId, SymbolId};

#[derive(Clone, Debug, Default)]
pub struct ScopeTree {
    scopes: Vec<Scope>,
}

impl ScopeTree {
    pub fn alloc(&mut self, parent: Option<ScopeId>, owner: ScopeOwner, span: Span) -> ScopeId {
        let id = ScopeId(self.scopes.len().min(u32::MAX as usize) as u32);
        self.scopes.push(Scope {
            id,
            parent,
            owner,
            symbols: Vec::new(),
            names: HashMap::new(),
            span,
        });
        id
    }

    pub fn insert(&mut self, scope: ScopeId, name: String, symbol: SymbolId) -> Option<SymbolId> {
        self.insert_in_namespace(scope, ScopeNamespace::Item, name, symbol)
    }

    pub fn insert_module(
        &mut self,
        scope: ScopeId,
        name: String,
        symbol: SymbolId,
    ) -> Option<SymbolId> {
        self.insert_in_namespace(scope, ScopeNamespace::Module, name, symbol)
    }

    pub fn insert_in_namespace(
        &mut self,
        scope: ScopeId,
        namespace: ScopeNamespace,
        name: String,
        symbol: SymbolId,
    ) -> Option<SymbolId> {
        let scope = &mut self.scopes[scope.index()];
        let entry = scope.names.entry((namespace, name)).or_default();
        let previous = entry.first().copied();
        entry.push(symbol);
        if !scope.symbols.contains(&symbol) {
            scope.symbols.push(symbol);
        }
        previous
    }

    pub fn lookup_local(&self, scope: ScopeId, name: &str) -> Option<SymbolId> {
        self.lookup_local_in_namespace(scope, ScopeNamespace::Item, name)
    }

    pub fn lookup_local_in_namespace(
        &self,
        scope: ScopeId,
        namespace: ScopeNamespace,
        name: &str,
    ) -> Option<SymbolId> {
        self.scopes[scope.index()]
            .names
            .get(&(namespace, name.to_owned()))
            .and_then(|symbols| match symbols.as_slice() {
                [symbol] => Some(*symbol),
                _ => None,
            })
    }

    pub fn lookup(&self, scope: ScopeId, name: &str) -> ResolveResult {
        self.lookup_in_namespace(scope, ScopeNamespace::Item, name)
    }

    pub fn lookup_module(&self, scope: ScopeId, name: &str) -> ResolveResult {
        self.lookup_in_namespace(scope, ScopeNamespace::Module, name)
    }

    pub fn lookup_in_namespace(
        &self,
        scope: ScopeId,
        namespace: ScopeNamespace,
        name: &str,
    ) -> ResolveResult {
        let mut current = Some(scope);
        while let Some(scope_id) = current {
            let scope = &self.scopes[scope_id.index()];
            if let Some(symbols) = scope.names.get(&(namespace, name.to_owned())) {
                return match symbols.as_slice() {
                    [symbol] => ResolveResult::Resolved(*symbol),
                    [] => ResolveResult::Unresolved,
                    many => ResolveResult::Ambiguous(many.to_vec()),
                };
            }
            current = scope.parent;
        }
        ResolveResult::Unresolved
    }

    pub fn get(&self, id: ScopeId) -> Option<&Scope> {
        self.scopes.get(id.index())
    }

    pub fn iter(&self) -> impl Iterator<Item = &Scope> {
        self.scopes.iter()
    }
}

#[derive(Clone, Debug)]
pub struct Scope {
    pub id: ScopeId,
    pub parent: Option<ScopeId>,
    pub owner: ScopeOwner,
    pub symbols: Vec<SymbolId>,
    names: HashMap<(ScopeNamespace, String), Vec<SymbolId>>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScopeNamespace {
    Item,
    Module,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeOwner {
    Module(HirModuleId),
    Item(HirItemId),
    Block(HirBlockId),
    Lambda(HirExprId),
    Handler(HirExprId),
    MatchArm(HirExprId),
}
