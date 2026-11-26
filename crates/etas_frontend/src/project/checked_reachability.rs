use etas_hir::{
    ExprView, HirExpr, HirExprId, HirItem, HirItemId, HirVisitor, ResolveResult, SymbolDef,
    SymbolId, walk_expr, walk_expr_tree, walk_item,
};
use std::collections::{BTreeSet, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckedCallableRoot {
    /// An item body, including a checked top-level value initializer.
    Item(HirItemId),
    Lambda(HirExprId),
    StdReference(HirExprId),
    ParameterDefault(SymbolId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckedExecutionReachability {
    pub items: BTreeSet<HirItemId>,
    pub default_expressions: BTreeSet<HirExprId>,
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
    InvalidParameterDefault(SymbolId),
    MissingParameterDefaultAnalysis(SymbolId),
}

impl std::fmt::Display for CheckedReachabilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "checked callable reachability failed: {self:?}")
    }
}
impl std::error::Error for CheckedReachabilityError {}

impl crate::CheckedProgramData {
    pub fn checked_parameter_default(
        &self,
        parameter: SymbolId,
    ) -> Result<crate::code::parameter_defaults::ParameterDefaultUnit, CheckedReachabilityError>
    {
        let invalid = || CheckedReachabilityError::InvalidParameterDefault(parameter);
        let Some(SymbolDef::Param { owner, .. }) =
            self.symbols.get(parameter).map(|symbol| &symbol.def)
        else {
            return Err(invalid());
        };
        let expression = *self
            .hir
            .parameter_defaults
            .get(&parameter)
            .ok_or_else(invalid)?;
        let Some(etas_types::SymbolTypeFact::Param { ty: output }) =
            self.types.symbol_types.get(&parameter)
        else {
            return Err(invalid());
        };
        let scope = self
            .hir
            .scopes
            .iter()
            .find(|scope| scope.symbols.contains(&parameter))
            .map(|scope| scope.id)
            .ok_or_else(invalid)?;
        let unit = etas_effects::EffectUnit::ParameterDefault {
            owner: *owner,
            parameter,
            expr: expression,
        };
        let summary = self.effects.unit_effects.get(&unit).ok_or(
            CheckedReachabilityError::MissingParameterDefaultAnalysis(parameter),
        )?;
        if matches!(
            summary.support,
            etas_effects::InterpreterSupport::Rejected(_)
        ) || (!self.hir.parameter_injections.contains(&parameter)
            && (summary.determinism != etas_effects::Determinism::Deterministic
                || !summary.escaping_effects.is_empty()
                || !summary.requested_actions.is_empty()
                || !summary.default_actions.is_empty()
                || !summary.handled_actions.is_empty()
                || matches!(
                    summary.support,
                    etas_effects::InterpreterSupport::Rejected(_)
                )
                || !self
                    .effects
                    .suspension
                    .get(&unit)
                    .is_some_and(|fact| fact.proven_non_suspending())))
        {
            return Err(invalid());
        }
        Ok(crate::code::parameter_defaults::ParameterDefaultUnit {
            parameter,
            owner: *owner,
            expression,
            scope,
            output: *output,
        })
    }

    pub fn checked_callable_owner(
        &self,
        root: CheckedCallableRoot,
    ) -> Result<HirItemId, CheckedReachabilityError> {
        match root {
            CheckedCallableRoot::ParameterDefault(parameter) => self
                .checked_parameter_default(parameter)
                .map(|unit| unit.owner),
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
                    Some(etas_types::ItemSignature::TopLevelLet(_)) => {
                        matches!(self.hir.items.get(item), Some(HirItem::TopLevelLet(_)))
                    }
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

    pub fn checked_callable_expression(
        &self,
        root: CheckedCallableRoot,
    ) -> Result<Option<HirExprId>, CheckedReachabilityError> {
        self.checked_callable_owner(root)?;
        Ok(match root {
            CheckedCallableRoot::Item(_) => None,
            CheckedCallableRoot::Lambda(expr) | CheckedCallableRoot::StdReference(expr) => {
                Some(expr)
            }
            CheckedCallableRoot::ParameterDefault(parameter) => {
                Some(self.checked_parameter_default(parameter)?.expression)
            }
        })
    }

    pub fn checked_callable_effects(
        &self,
        root: CheckedCallableRoot,
    ) -> Result<&etas_effects::EffectSummary, CheckedReachabilityError> {
        let owner = self.checked_callable_owner(root)?;
        match root {
            CheckedCallableRoot::ParameterDefault(parameter) => {
                let expr = self.checked_parameter_default(parameter)?.expression;
                self.effects
                    .unit_effects
                    .get(&etas_effects::EffectUnit::ParameterDefault {
                        owner,
                        parameter,
                        expr,
                    })
                    .ok_or(CheckedReachabilityError::MissingParameterDefaultAnalysis(
                        parameter,
                    ))
            }
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

    /// Returns execution suspension facts independently of escaping effects.
    /// Missing imported/primitive evidence is unknown, not a non-suspension proof.
    pub fn checked_callable_suspension(
        &self,
        root: CheckedCallableRoot,
    ) -> Result<etas_effects::suspension::SuspensionSummary, CheckedReachabilityError> {
        let owner = self.checked_callable_owner(root)?;
        self.checked_callable_effects(root)?;
        let unit = match root {
            CheckedCallableRoot::ParameterDefault(parameter) => {
                etas_effects::EffectUnit::ParameterDefault {
                    owner,
                    parameter,
                    expr: self.checked_parameter_default(parameter)?.expression,
                }
            }
            CheckedCallableRoot::Item(item) => etas_effects::EffectUnit::Item(item),
            CheckedCallableRoot::Lambda(value) => {
                let body = match self.effects.latent_effects[&value].body {
                    etas_effects::LatentEffectBodyRef::Expr(expr) => {
                        etas_effects::infer::unit::EffectAnonymousFlowBody::Expr(expr)
                    }
                    etas_effects::LatentEffectBodyRef::Block(block) => {
                        etas_effects::infer::unit::EffectAnonymousFlowBody::Block(block)
                    }
                };
                etas_effects::EffectUnit::AnonymousFlow { owner, value, body }
            }
            CheckedCallableRoot::StdReference(_) => {
                return Ok(etas_effects::suspension::SuspensionSummary::UNKNOWN);
            }
        };
        Ok(self
            .effects
            .suspension
            .get(&unit)
            .copied()
            .unwrap_or(etas_effects::suspension::SuspensionSummary::UNKNOWN))
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

    /// Direct checked references, including defaults, methods and implicit packs.
    pub(crate) fn direct_checked_dependencies(
        &self,
        item: HirItemId,
    ) -> Result<BTreeSet<HirItemId>, CheckedReachabilityError> {
        let mut pending = Vec::new();
        if let Some(method) = self.types.agent_default_method(item) {
            pending.push(method.body);
        }
        let mut defaults_seen = HashSet::new();
        let mut visitor = References {
            checked: self,
            pending: &mut pending,
            error: None,
            defaults_seen: &mut defaults_seen,
        };
        walk_item(&self.hir.view(), item, &mut visitor);
        for annotation in self.hir.execution_annotations(item) {
            for argument in &annotation.args {
                let value = match argument {
                    etas_hir::HirAnnotationArg::Positional { value, .. }
                    | etas_hir::HirAnnotationArg::Named { value, .. } => *value,
                };
                walk_expr_tree(&self.hir.view(), value, &mut visitor);
            }
        }
        if let Some(error) = visitor.error {
            return Err(error);
        }
        Ok(pending.into_iter().collect())
    }

    pub fn checked_callable_reachability(
        &self,
        root: CheckedCallableRoot,
    ) -> Result<BTreeSet<HirItemId>, CheckedReachabilityError> {
        self.checked_execution_reachability(root)
            .map(|closure| closure.items)
    }

    pub fn checked_execution_reachability(
        &self,
        root: CheckedCallableRoot,
    ) -> Result<CheckedExecutionReachability, CheckedReachabilityError> {
        self.checked_callable_effects(root)?;
        let view = self.hir.view();
        let mut visited = BTreeSet::new();
        let mut pending = Vec::new();
        let mut defaults_seen = HashSet::new();
        match root {
            CheckedCallableRoot::Item(item) => pending.push(item),
            CheckedCallableRoot::Lambda(_)
            | CheckedCallableRoot::StdReference(_)
            | CheckedCallableRoot::ParameterDefault(_) => {
                let expr = match root {
                    CheckedCallableRoot::Lambda(expr) | CheckedCallableRoot::StdReference(expr) => {
                        expr
                    }
                    CheckedCallableRoot::ParameterDefault(parameter) => {
                        defaults_seen.insert(parameter);
                        self.checked_parameter_default(parameter)?.expression
                    }
                    _ => unreachable!(),
                };
                let mut visitor = References {
                    checked: self,
                    pending: &mut pending,
                    error: None,
                    defaults_seen: &mut defaults_seen,
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
                defaults_seen: &mut defaults_seen,
            };
            walk_item(&view, item, &mut visitor);
            for annotation in self.hir.execution_annotations(item) {
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
        Ok(CheckedExecutionReachability {
            items: visited,
            default_expressions: defaults_seen
                .iter()
                .map(|parameter| self.hir.parameter_defaults[parameter])
                .collect(),
        })
    }
}

struct References<'a> {
    checked: &'a crate::CheckedProgramData,
    pending: &'a mut Vec<HirItemId>,
    error: Option<CheckedReachabilityError>,
    defaults_seen: &'a mut HashSet<SymbolId>,
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
        let checked = self.checked;
        let defaults = checked
            .types
            .checked_method_calls
            .get(&expression.id())
            .map(|method| method.defaults.as_slice())
            .or_else(|| {
                self.checked
                    .types
                    .declared_call_arguments
                    .get(&expression.id())
                    .map(|call| call.defaults.as_slice())
            })
            .unwrap_or(&[])
            .iter()
            .filter_map(|default| match &default.source {
                etas_types::ParameterDefaultRef::Source { parameter } => Some(*parameter),
                etas_types::ParameterDefaultRef::External { .. } => None,
            })
            .collect::<Vec<_>>();
        for parameter in defaults {
            if !self.defaults_seen.insert(parameter) {
                continue;
            }
            match checked.checked_parameter_default(parameter) {
                Ok(unit) => walk_expr_tree(&checked.hir.view(), unit.expression, self),
                Err(error) => {
                    self.error = Some(error);
                    return;
                }
            }
            if self.error.is_some() {
                return;
            }
        }
        if let Some(pack) = self.checked.types.existential_packs.get(&expression.id()) {
            for witness in &pack.witnesses {
                if let etas_types::method::MethodTarget::Source { callable, .. }
                | etas_types::method::MethodTarget::Agent { callable, .. } = witness.target
                {
                    self.pending.push(callable);
                }
            }
        }
        if let Some(method) = self
            .checked
            .types
            .checked_method_calls
            .get(&expression.id())
            .or_else(|| {
                self.checked
                    .types
                    .bound_method_values
                    .get(&expression.id())
                    .map(|value| &value.method)
            })
        {
            match method.target {
                etas_types::method::MethodTarget::Source { callable, .. }
                | etas_types::method::MethodTarget::Agent { callable, .. } => {
                    self.pending.push(callable)
                }
                _ => {}
            }
        }
        if self
            .checked
            .types
            .checked_method_calls
            .get(&expression.id())
            .is_some_and(|method| {
                matches!(
                    method.target,
                    etas_types::method::MethodTarget::BoundSpec { .. }
                        | etas_types::method::MethodTarget::External { .. }
                )
            })
        {
            let deferred = self.checked.effects.contract_method_calls.contains_key(&expression.id())
                || self.checked.effects.expr_effects.get(&expression.id()).is_some_and(|summary|
                summary.action_trace.parameter_calls().iter().any(|call|
                    matches!(call.target, etas_effects::ParameterCallTarget::SpecMethod { call, .. } if call == expression.id())));
            let metadata = self
                .checked
                .effects
                .metadata_call_targets
                .get(&expression.id())
                .is_some_and(|targets| {
                    !targets.is_empty()
                        && targets.iter().all(|target| {
                            matches!(
                                self.checked.effects.metadata_callables.get(target),
                                Some(etas_effects::facts::MetadataCallableAnalysis::Solved { .. })
                            )
                        })
                });
            let Some(targets) = self
                .checked
                .effects
                .native_call_targets
                .get(&expression.id())
            else {
                // A generic method is linked when its checked binding is applied.
                if !deferred && !metadata {
                    self.error = Some(CheckedReachabilityError::MissingSpecMethod(expression.id()));
                }
                return;
            };
            if targets.is_empty() && !deferred && !metadata {
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
