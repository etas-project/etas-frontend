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

    pub fn track_bindings(&mut self, bindings: HashSet<SymbolId>) {
        self.affine_symbols.clone_from(&bindings);
        self.lease_bindings = bindings;
    }

    pub fn index(&mut self, hir: &HirRevision, types: &TypeOutput, bindings: HashSet<SymbolId>) {
        self.track_bindings(bindings);
        if self.lease_bindings.is_empty() {
            return;
        }
        for scope in hir.scopes.iter() {
            self.declarations
                .extend(scope.symbols.iter().map(|s| (*s, scope.id)));
        }
        self.callbacks.extend(lease_callbacks(hir, types));
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
                && !self.retained(hir, value, &LeaseState::default()).is_empty()
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

    pub fn retained(
        &self,
        hir: &HirRevision,
        expr: HirExprId,
        state: &LeaseState,
    ) -> HashSet<SymbolId> {
        retained_bindings(
            hir,
            self.layouts,
            expr,
            |s| state.captured.get(&s),
            |symbol| self.lease_bindings.contains(&symbol),
        )
    }

    pub fn is_affine(&self, symbol: SymbolId, state: &LeaseState) -> bool {
        self.lease_bindings.contains(&symbol)
            || state.captured.get(&symbol).map_or_else(
                || self.affine_symbols.contains(&symbol),
                |origins| !origins.is_empty(),
            )
    }

    pub fn is_affine_place(&self, hir: &HirProgram, expr: HirExprId, state: &LeaseState) -> bool {
        matches!(place_of(hir, expr), Some(Place { root: AliasTarget::Symbol(symbol), projections })
            if projections.is_empty() && self.is_affine(symbol, state))
    }

    pub fn check_return(&mut self, hir: &HirRevision, value: HirExprId, state: &LeaseState) {
        if let Some(scope) = self.current
            && self.retained(hir, value, state).iter().any(|origin| {
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
        span: Span,
        origins: &HashSet<SymbolId>,
    ) {
        let Some(Place {
            root: AliasTarget::Symbol(root),
            projections,
        }) = place_of(hir, target)
        else {
            return;
        };
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

fn lease_callbacks(
    hir: &HirRevision,
    types: &TypeOutput,
) -> impl Iterator<Item = (HirBodyRef, ScopeId)> {
    hir.exprs.iter().filter_map(|(expr, node)| {
        let HirExpr::Call { args, .. } = node else {
            return None;
        };
        if !is_with_lease(hir, types, expr) {
            return None;
        }
        let arg = args.iter().enumerate().find_map(|(index, arg)| match arg {
            HirArg::Named { name, value, .. } if name == "body" => Some(*value),
            HirArg::Positional(value) if index == 1 => Some(*value),
            _ => None,
        })?;
        callback(hir, arg)
    })
}

/// The effect solver substitutes call arguments into returned callable origins,
/// including those loaded from a dependency's checked package interface.
pub(crate) fn returned_captures(
    hir: &HirRevision,
    types: &TypeOutput,
    effects: &etas_effects::EffectFacts,
) -> Result<Vec<Diagnostic>, String> {
    use etas_effects::{
        CallableOutcome, CallableReturnSource, EffectUnit, ProjectedReturnSources,
        ReturnObservation,
    };
    let callbacks: HashMap<_, _> = lease_callbacks(hir, types).collect();
    if callbacks.is_empty() {
        return Ok(Vec::new());
    }
    let Some(lease) = lease_type(types) else {
        return Ok(Vec::new());
    };
    let layouts = etas_hir_analysis::closures::analyze_runtime_captures(&hir.view())?;
    let mut analysis = LeaseAnalysis::new(HirAnalysisContext::new(hir), types, lease, &layouts);
    let mut declarations = HashMap::new();
    for scope in hir.scopes.iter() {
        for symbol in &scope.symbols {
            if types
                .facts
                .symbol_types
                .get(symbol)
                .and_then(value_type)
                .is_some_and(|ty| analysis.contains_lease(ty))
            {
                declarations.insert(*symbol, scope.id);
            }
        }
    }
    let mut escapes = HashSet::new();
    for (unit, summary) in &effects.unit_effects {
        let body = match *unit {
            EffectUnit::AnonymousFlow { owner, value, .. } => {
                HirBodyRef::new(owner, HirBodyKind::Lambda(value))
            }
            EffectUnit::Item(item) => {
                let Some(body) = hir.index().item_body.get(&item) else {
                    continue;
                };
                *body
            }
            _ => continue,
        };
        let (Some(scope), Some(exits)) = (callbacks.get(&body), summary.exits.as_ref()) else {
            continue;
        };
        let sources = exits
            .exits
            .iter()
            .filter_map(|exit| match &exit.outcome {
                CallableOutcome::Returned {
                    observation: ReturnObservation::CallableSources(sources),
                    ..
                } => Some(sources),
                _ => None,
            })
            .chain(
                exits
                    .value_projections
                    .values()
                    .filter_map(|value| match value {
                        ProjectedReturnSources::Known(sources) => Some(sources),
                        ProjectedReturnSources::Unavailable => None,
                    }),
            )
            .flatten();
        for source in sources {
            let (CallableReturnSource::AnonymousFlow(origin)
            | CallableReturnSource::Captured { unit: origin, .. }) = source
            else {
                continue;
            };
            let EffectUnit::AnonymousFlow { value, .. } = *origin else {
                continue;
            };
            let origins = retained_bindings(
                hir,
                &layouts,
                value,
                |_| None,
                |symbol| {
                    declarations
                        .get(&symbol)
                        .is_some_and(|decl| within(hir, *scope, *decl))
                },
            );
            if !origins.is_empty() {
                escapes.insert(hir.exprs[value].span(&hir.blocks));
            }
        }
    }
    let mut diagnostics: Vec<_> = escapes
        .into_iter()
        .map(|span| {
            Diagnostic::type_check(
                TypeDiagnosticCode::TypeMismatch,
                span,
                "returned callable retains the SessionLease beyond its owning with_lease scope",
            )
        })
        .collect();
    diagnostics.sort_by_key(|d| (d.primary.span.source.0, d.primary.span.range.start));
    Ok(diagnostics)
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
