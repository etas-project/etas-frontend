use super::*;
use crate::passes::analysis::lexical_capture::retained_bindings;
use etas_hir::{HirBodyKind, HirBodyRef, HirNodeRef, HirPat, ScopeId};

pub(super) struct LeaseCaptures<'a> {
    pub layouts: &'a CaptureLayouts,
    pub affine_symbols: HashSet<SymbolId>,
    lease_bindings: HashSet<SymbolId>,
    pub callbacks: HashMap<HirBodyRef, ScopeId>,
    declarations: HashMap<SymbolId, ScopeId>,
    pub escapes: HashSet<Span>,
    current: Option<ScopeId>,
    pub tail: Option<HirExprId>,
}

impl<'a> LeaseCaptures<'a> {
    pub fn new(layouts: &'a CaptureLayouts) -> Self {
        Self {
            layouts,
            affine_symbols: HashSet::new(),
            lease_bindings: HashSet::new(),
            callbacks: HashMap::new(),
            declarations: HashMap::new(),
            escapes: HashSet::new(),
            current: None,
            tail: None,
        }
    }

    pub fn index(&mut self, hir: &HirRevision, types: &TypeOutput, bindings: HashSet<SymbolId>) {
        self.lease_bindings = bindings;
        if self.lease_bindings.is_empty() {
            return;
        }
        self.affine_symbols.clone_from(&self.lease_bindings);
        for scope in hir.scopes.iter() {
            self.declarations
                .extend(scope.symbols.iter().map(|s| (*s, scope.id)));
        }
        for (expr, node) in hir.exprs.iter() {
            if let HirExpr::Call { args, .. } = node
                && is_with_lease(hir, types, expr)
                && let Some(arg) = args.iter().enumerate().find_map(|(index, arg)| match arg {
                    HirArg::Named { name, value, .. } if name == "body" => Some(*value),
                    HirArg::Positional(value) if index == 1 => Some(*value),
                    _ => None,
                })
                && let Some((body, scope)) = callback(hir, arg)
            {
                self.callbacks.insert(body, scope);
            }
        }
        let owners: HashSet<_> = self
            .layouts
            .iter()
            .filter_map(|(expr, layout)| {
                layout
                    .captures
                    .iter()
                    .any(|s| self.lease_bindings.contains(s))
                    .then(|| hir.index().expr_item[expr])
            })
            .collect();
        for symbol in hir.symbols.iter() {
            if let SymbolDef::Local {
                binding,
                initializer: Some(value),
                ..
            } = symbol.def
                && hir
                    .index()
                    .stmt_item
                    .get(&binding)
                    .is_some_and(|item| owners.contains(item))
                && let HirStmt::Let { pat, .. } | HirStmt::Var { pat, .. } = hir.stmts[binding]
                && matches!(hir.pats[pat], HirPat::Binding { symbol: id, .. } if id == symbol.id)
                && !self.retained(hir, value).is_empty()
            {
                self.affine_symbols.insert(symbol.id);
            }
        }
    }

    pub fn enter_body(&mut self, hir: &HirRevision, body: HirBodyRef) {
        self.current = self.callbacks.get(&body).copied();
        self.tail = self.current.and_then(|_| {
            let index = hir.index();
            index
                .body_exprs
                .get(&body)
                .and_then(|v| v.first().copied())
                .or_else(|| hir.blocks[*index.body_blocks.get(&body)?.first()?].final_expr)
        });
    }

    fn retained(&self, hir: &HirRevision, expr: HirExprId) -> HashSet<SymbolId> {
        retained_bindings(hir, self.layouts, expr, |symbol| {
            self.lease_bindings.contains(&symbol)
        })
    }

    pub fn is_affine_place(&self, hir: &HirProgram, expr: HirExprId) -> bool {
        matches!(place_of(hir, expr), Some(Place { root: AliasTarget::Symbol(symbol), projections })
            if projections.is_empty() && self.affine_symbols.contains(&symbol))
    }

    pub fn check_return(&mut self, hir: &HirRevision, value: HirExprId) {
        if let Some(scope) = self.current
            && self.retained(hir, value).iter().any(|origin| {
                self.declarations
                    .get(origin)
                    .is_some_and(|decl| within(hir, scope, *decl))
            })
        {
            self.escapes.insert(hir.exprs[value].span(&hir.blocks));
        }
    }

    pub fn check_store(
        &mut self,
        hir: &HirRevision,
        types: &TypeOutput,
        stmt: HirStmtId,
        target: HirExprId,
        value: HirExprId,
        typed_lease: bool,
        span: Span,
    ) {
        let Some(Place {
            root: AliasTarget::Symbol(root),
            projections,
        }) = place_of(hir, target)
        else {
            return;
        };
        let mut origins = self.retained(hir, value);
        if let Some(Place {
            root: AliasTarget::Symbol(symbol),
            ..
        }) = place_of(hir, value)
            && self.lease_bindings.contains(&symbol)
            && typed_lease
        {
            origins.insert(symbol);
        }
        if origins.is_empty() {
            return;
        }
        let agent_state = !projections.is_empty()
            && types
                .facts
                .symbol_types
                .get(&root)
                .and_then(value_type)
                .is_some_and(|ty| types.facts.is_active_agent(&types.store, ty));
        let stmt_scope = hir.view().enclosing_scope(HirNodeRef::Stmt(stmt));
        let escapes_callback = self.callbacks.values().any(|scope| {
            stmt_scope.is_some_and(|location| within(hir, *scope, location))
                && !self
                    .declarations
                    .get(&root)
                    .is_some_and(|target| within(hir, *scope, *target))
                && origins.iter().any(|s| {
                    self.declarations
                        .get(s)
                        .is_some_and(|origin| within(hir, *scope, *origin))
                })
        });
        if agent_state || escapes_callback {
            self.escapes.insert(span);
        }
    }
}

fn callback(hir: &HirRevision, mut expr: HirExprId) -> Option<(HirBodyRef, ScopeId)> {
    let mut seen = HashSet::new();
    while seen.insert(expr) {
        match &hir.exprs[expr] {
            HirExpr::Lambda { scope, .. } => {
                return Some((
                    HirBodyRef::new(hir.index().expr_item[&expr], HirBodyKind::Lambda(expr)),
                    *scope,
                ));
            }
            HirExpr::Path(path) => {
                let ResolveResult::Resolved(symbol) = path.resolution else {
                    return None;
                };
                match hir.symbols.get(hir.canonical_source_symbol(symbol)?)?.def {
                    SymbolDef::Local {
                        binding,
                        initializer: Some(value),
                        ..
                    } if matches!(hir.stmts[binding], HirStmt::Let { .. }) => expr = value,
                    SymbolDef::TopLevelLet { initializer, .. } => expr = initializer,
                    SymbolDef::Item { item } => {
                        let etas_hir::HirItem::Flow(flow) = &hir.items[item] else {
                            return None;
                        };
                        return Some((*hir.index().item_body.get(&item)?, flow.scope));
                    }
                    _ => return None,
                }
            }
            _ => return None,
        }
    }
    None
}

fn within(hir: &HirProgram, ancestor: ScopeId, mut scope: ScopeId) -> bool {
    loop {
        if scope == ancestor {
            return true;
        }
        let Some(parent) = hir.scopes.get(scope).and_then(|s| s.parent) else {
            return false;
        };
        scope = parent;
    }
}
