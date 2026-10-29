use etas_hir::{
    ExprView, HirExpr, HirExprId, HirItem, HirItemId, HirVisitor, ResolveResult, SymbolDef,
    SymbolId, walk_expr, walk_expr_tree, walk_item,
};
use std::collections::{BTreeSet, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckedCallableRoot {
    Item(HirItemId),
    Lambda(HirExprId),
    StdReference(HirExprId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckedReachabilityError {
    InvalidEntry(HirItemId),
    MissingItem(HirItemId),
    MissingAnalysis(HirItemId),
    MissingSymbol(SymbolId),
    MissingMethod(SymbolId),
    MissingSpecMethod(HirExprId),
    AliasCycle(SymbolId),
    UnresolvedPath(HirExprId),
    InvalidLambda(HirExprId),
    MissingLambdaAnalysis(HirExprId),
    InconsistentLambdaFacts(HirExprId),
    MissingResourceFact(SymbolId),
    InvalidStdReference(HirExprId),
}

impl std::fmt::Display for CheckedReachabilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "checked callable reachability failed: {self:?}")
    }
}
impl std::error::Error for CheckedReachabilityError {}

impl crate::CheckedProgramData {
    pub fn checked_callable_owner(
        &self,
        root: CheckedCallableRoot,
    ) -> Result<HirItemId, CheckedReachabilityError> {
        match root {
            CheckedCallableRoot::StdReference(expr) => {
                let invalid = || CheckedReachabilityError::InvalidStdReference(expr);
                let fact = self
                    .effects
                    .std_callable_values
                    .get(&expr)
                    .ok_or_else(invalid)?;
                let Some(HirExpr::Path(path)) = self.hir.exprs.get(expr) else {
                    return Err(invalid());
                };
                if path.resolution != ResolveResult::Resolved(fact.symbol)
                    || fact.value != expr
                    || self.types.expr_types.get(&expr) != Some(&fact.flow_type)
                    || !matches!(
                        self.type_store.get(fact.flow_type),
                        Some(etas_types::Type::Function(_))
                    )
                {
                    return Err(invalid());
                }
                let Some(SymbolDef::ImportAlias { path, .. }) =
                    self.symbols.get(fact.symbol).map(|symbol| &symbol.def)
                else {
                    return Err(invalid());
                };
                let intrinsic = self
                    .std_registry
                    .lookup_qualified(path)
                    .and_then(|symbol| symbol.intrinsic.as_ref())
                    .ok_or_else(invalid)?;
                if intrinsic.id.0 != fact.intrinsic {
                    return Err(invalid());
                }
                self.hir
                    .index()
                    .expr_item
                    .get(&expr)
                    .copied()
                    .ok_or_else(invalid)
            }
            CheckedCallableRoot::Item(item) => {
                let callable = match self.types.item_signatures.get(&item) {
                    Some(
                        etas_types::ItemSignature::Flow(_) | etas_types::ItemSignature::Tool(_),
                    ) => true,
                    Some(etas_types::ItemSignature::Agent(agent)) => agent.callable().is_some(),
                    _ => false,
                };
                if !callable {
                    return Err(CheckedReachabilityError::InvalidEntry(item));
                }
                Ok(item)
            }
            CheckedCallableRoot::Lambda(expr) => {
                let Some(HirExpr::Lambda { body, params, .. }) = self.hir.exprs.get(expr) else {
                    return Err(CheckedReachabilityError::InvalidLambda(expr));
                };
                let fact = self
                    .effects
                    .latent_effects
                    .get(&expr)
                    .ok_or(CheckedReachabilityError::MissingLambdaAnalysis(expr))?;
                let body = match body {
                    etas_hir::HirLambdaBody::Expr(body) => {
                        etas_effects::LatentEffectBodyRef::Expr(*body)
                    }
                    etas_hir::HirLambdaBody::Block(body) => {
                        etas_effects::LatentEffectBodyRef::Block(*body)
                    }
                };
                let Some(etas_types::Type::Function(signature)) =
                    self.type_store.get(fact.flow_type)
                else {
                    return Err(CheckedReachabilityError::InconsistentLambdaFacts(expr));
                };
                if fact.value != expr
                    || fact.body != body
                    || signature.input.len() != params.len()
                    || self.types.expr_types.get(&expr) != Some(&fact.flow_type)
                    || fact.inferred != fact.summary.escaping_effects
                {
                    return Err(CheckedReachabilityError::InconsistentLambdaFacts(expr));
                }
                self.hir
                    .view()
                    .index()
                    .expr_item
                    .get(&expr)
                    .copied()
                    .ok_or(CheckedReachabilityError::InvalidLambda(expr))
            }
        }
    }

    pub fn checked_callable_effects(
        &self,
        root: CheckedCallableRoot,
    ) -> Result<&etas_effects::EffectSummary, CheckedReachabilityError> {
        let owner = self.checked_callable_owner(root)?;
        match root {
            CheckedCallableRoot::StdReference(expr) => self
                .effects
                .std_callable_values
                .get(&expr)
                .map(|fact| &fact.summary)
                .ok_or(CheckedReachabilityError::InvalidStdReference(expr)),
            CheckedCallableRoot::Item(item) => self
                .effects
                .item_effects
                .get(&item)
                .ok_or(CheckedReachabilityError::MissingAnalysis(owner)),
            CheckedCallableRoot::Lambda(expr) => self
                .effects
                .latent_effects
                .get(&expr)
                .map(|fact| &fact.summary)
                .ok_or(CheckedReachabilityError::MissingLambdaAnalysis(expr)),
        }
    }

    /// Selects the local executable closure within the same checked snapshot.
    /// Frozen Agent calls are boundaries owned by their source execution plan.
    /// This neither changes the project entry nor supplies missing facts.
    pub fn checked_call_reachability(
        &self,
        entry: HirItemId,
    ) -> Result<BTreeSet<HirItemId>, CheckedReachabilityError> {
        self.checked_callable_reachability(CheckedCallableRoot::Item(entry))
    }

    pub fn checked_callable_reachability(
        &self,
        root: CheckedCallableRoot,
    ) -> Result<BTreeSet<HirItemId>, CheckedReachabilityError> {
        self.checked_callable_effects(root)?;
        let view = self.hir.view();
        let mut visited = BTreeSet::new();
        let mut pending = Vec::new();
        match root {
            CheckedCallableRoot::Item(item) => pending.push(item),
            CheckedCallableRoot::Lambda(expr) | CheckedCallableRoot::StdReference(expr) => {
                let mut visitor = References {
                    checked: self,
                    pending: &mut pending,
                    error: None,
                };
                walk_expr_tree(&view, expr, &mut visitor);
                if let Some(error) = visitor.error {
                    return Err(error);
                }
            }
        }
        while let Some(item) = pending.pop() {
            if root != CheckedCallableRoot::Item(item)
                && self.frozen_agent_call_boundaries.contains(&item)
            {
                continue;
            }
            if !visited.insert(item) {
                continue;
            }
            let declaration = self
                .hir
                .items
                .get(item)
                .ok_or(CheckedReachabilityError::MissingItem(item))?;
            if matches!(
                declaration,
                HirItem::Flow(_) | HirItem::Tool(_) | HirItem::TopLevelLet(_)
            ) && !self.effects.item_effects.contains_key(&item)
            {
                return Err(CheckedReachabilityError::MissingAnalysis(item));
            }
            if let Some(etas_types::ItemSignature::Agent(agent)) =
                self.types.item_signatures.get(&item)
                && let Some(entry) = &agent.default_entry
            {
                let symbol = entry
                    .method
                    .ok_or(CheckedReachabilityError::InvalidEntry(item))?;
                let method = self
                    .types
                    .agent_methods
                    .get(&symbol)
                    .ok_or(CheckedReachabilityError::MissingMethod(symbol))?;
                if method.agent != item {
                    return Err(CheckedReachabilityError::MissingMethod(symbol));
                }
                if method.body != item {
                    pending.push(method.body);
                }
                if !self.effects.item_effects.contains_key(&method.body) {
                    return Err(CheckedReachabilityError::MissingAnalysis(method.body));
                }
            }
            let mut visitor = References {
                checked: self,
                pending: &mut pending,
                error: None,
            };
            walk_item(&view, item, &mut visitor);
            for annotation in self.hir.item_annotations.get(&item).into_iter().flatten() {
                for argument in &annotation.args {
                    let value = match argument {
                        etas_hir::HirAnnotationArg::Positional { value, .. }
                        | etas_hir::HirAnnotationArg::Named { value, .. } => *value,
                    };
                    walk_expr(&view, value, &mut visitor);
                }
            }
            if let Some(error) = visitor.error {
                return Err(error);
            }
        }
        Ok(visited)
    }
}

struct References<'a> {
    checked: &'a crate::CheckedProgramData,
    pending: &'a mut Vec<HirItemId>,
    error: Option<CheckedReachabilityError>,
}

impl References<'_> {
    fn symbol(&mut self, mut symbol: SymbolId) -> Result<(), CheckedReachabilityError> {
        let mut seen = HashSet::new();
        loop {
            if !seen.insert(symbol) {
                return Err(CheckedReachabilityError::AliasCycle(symbol));
            }
            let definition = self
                .checked
                .hir
                .symbols
                .get(symbol)
                .ok_or(CheckedReachabilityError::MissingSymbol(symbol))?;
            if let Some(target) = self.checked.hir.source_import_targets.get(&symbol) {
                symbol = *target;
                continue;
            }
            match definition.def {
                SymbolDef::TopLevelLet {
                    classification: etas_hir::TopLevelLetClassification::ResourceHandle(_),
                    ..
                } => {
                    if !self.checked.types.resource_handles.contains_key(&symbol) {
                        return Err(CheckedReachabilityError::MissingResourceFact(symbol));
                    }
                }
                SymbolDef::Item { item } | SymbolDef::TopLevelLet { item, .. } => {
                    self.pending.push(item)
                }
                _ => {}
            }
            return Ok(());
        }
    }
}

impl<'view, 'hir> HirVisitor<'view, 'hir> for References<'_> {
    fn enter_expr(&mut self, expression: ExprView<'view, 'hir>) {
        if self.error.is_some() {
            return;
        }
        if matches!(expression.data(), HirExpr::SpecMethodCall { .. }) {
            let Some(targets) = self
                .checked
                .effects
                .native_call_targets
                .get(&expression.id())
            else {
                self.error = Some(CheckedReachabilityError::MissingSpecMethod(expression.id()));
                return;
            };
            if targets.is_empty() {
                self.error = Some(CheckedReachabilityError::MissingSpecMethod(expression.id()));
                return;
            }
            for target in targets {
                let etas_effects::EffectUnit::Item(item) = target else {
                    self.error = Some(CheckedReachabilityError::MissingSpecMethod(expression.id()));
                    return;
                };
                self.pending.push(*item);
            }
        }
        if let Some(symbol) = self.checked.types.agent_method_calls.get(&expression.id()) {
            let Some(method) = self.checked.types.agent_methods.get(symbol) else {
                self.error = Some(CheckedReachabilityError::MissingMethod(*symbol));
                return;
            };
            self.pending.push(method.body);
        }
        let HirExpr::Path(path) = expression.data() else {
            return;
        };
        let symbol = match &path.resolution {
            ResolveResult::Resolved(symbol) => Some(*symbol),
            ResolveResult::PartiallyResolved(partial) => partial.resolved_prefix,
            _ => {
                self.error = Some(CheckedReachabilityError::UnresolvedPath(expression.id()));
                return;
            }
        };
        if let Some(symbol) = symbol
            && let Err(error) = self.symbol(symbol)
        {
            self.error = Some(error);
        }
    }
}
