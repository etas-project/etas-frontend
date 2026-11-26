use etas_hir::{HirExpr, HirExprId, HirLambdaBody, HirTreeChild, ResolveResult, SymbolId};
use std::collections::BTreeSet;

use super::{
    engine::EffectSemantics,
    state::{EffectState, LatentEnv, LatentSource},
};

#[derive(Clone, Default)]
pub(crate) struct LambdaReferences {
    symbols: BTreeSet<SymbolId>,
    value_paths: BTreeSet<HirExprId>,
}

impl EffectSemantics<'_> {
    pub(super) fn lambda_value_captures(
        &mut self,
        expr: HirExprId,
        state: &EffectState,
    ) -> Result<LatentEnv, &'static str> {
        let mut captures = LatentEnv::default();
        for symbol in self.lambda_referenced_symbols(expr)? {
            if let Some(sources) = state.local_latent_values.symbol_sources(symbol) {
                captures.record_symbol_sources(symbol, sources.iter().copied());
            }
            captures.symbol_projections.extend(
                state
                    .local_latent_values
                    .symbol_projections
                    .iter()
                    .filter(|((owner, _), _)| *owner == symbol)
                    .map(|(key, sources)| (key.clone(), sources.clone())),
            );
        }
        Ok(captures)
    }

    pub(super) fn lambda_referenced_symbols(
        &self,
        expr: HirExprId,
    ) -> Result<BTreeSet<SymbolId>, &'static str> {
        Ok(self.lambda_references(expr)?.symbols)
    }

    fn lambda_references(&self, expr: HirExprId) -> Result<LambdaReferences, &'static str> {
        if let Some(references) = self.lambda_capture_references.borrow().get(&expr) {
            return Ok(references.clone());
        }
        let references = self.collect_lambda_references(expr)?;
        self.lambda_capture_references
            .borrow_mut()
            .insert(expr, references.clone());
        Ok(references)
    }

    pub(super) fn lambda_projected_captures(
        &self,
        expr: HirExprId,
        state: &EffectState,
        parameters: &BTreeSet<SymbolId>,
    ) -> Result<BTreeSet<LatentSource>, &'static str> {
        let mut captures = BTreeSet::new();
        for value in self.lambda_references(expr)?.value_paths {
            for source in self
                .latent_sources_for_expr(state, value)
                .into_iter()
                .flatten()
            {
                if let LatentSource::Projection(id) = source
                    && self
                        .parameter_projection(id)
                        .is_some_and(|projection| parameters.contains(&projection.parameter))
                {
                    captures.insert(source);
                }
            }
        }
        // A returned closure can depend on a call's returned payload even
        // when the payload has no separate field expression in the lambda.
        let mut units = self
            .context
            .view()
            .index()
            .expr_item
            .get(&expr)
            .and_then(|owner| self.anonymous_flow_unit_for_expr(*owner, expr))
            .into_iter()
            .collect::<Vec<_>>();
        let mut seen_units = BTreeSet::new();
        let mut environments = Vec::new();
        while let Some(unit) = units.pop() {
            if !seen_units.insert(unit) {
                continue;
            }
            let Some(exits) = self.unit_summary(&unit)
                .and_then(|summary| summary.exits.as_ref())
            else {
                continue;
            };
            let mut returned = exits.callable_return_sources().unwrap_or_default();
            for sources in exits.value_projections.values() {
                if let crate::ProjectedReturnSources::Known(sources) = sources {
                    returned.extend(sources.iter().cloned());
                }
            }
            for source in returned {
                match source {
                    crate::CallableReturnSource::Projection(id) => {
                        if self
                            .parameter_projection(id)
                            .is_some_and(|projection| parameters.contains(&projection.parameter))
                        {
                            captures.insert(LatentSource::Projection(id));
                        }
                    }
                    crate::CallableReturnSource::Captured { environment, .. }
                    | crate::CallableReturnSource::Existential { environment, .. } => {
                        environments.push(environment)
                    }
                    crate::CallableReturnSource::AnonymousFlow(unit) => units.push(unit),
                    _ => {}
                }
            }
        }
        let mut seen_environments = BTreeSet::new();
        while let Some(environment) = environments.pop() {
            if !seen_environments.insert(environment) {
                continue;
            }
            let environment = self
                .callable_environment(environment)
                .map_err(|_| "returned callable capture environment is unavailable")?;
            for source in environment.parameters.into_values().flatten() {
                match source {
                    LatentSource::Projection(id)
                        if self.parameter_projection(id).is_some_and(|projection| {
                            parameters.contains(&projection.parameter)
                        }) =>
                    {
                        captures.insert(source);
                    }
                    LatentSource::Captured { environment, .. }
                    | LatentSource::Existential { environment, .. } => {
                        environments.push(environment)
                    }
                    _ => {}
                }
            }
        }
        Ok(captures)
    }

    fn collect_lambda_references(&self, expr: HirExprId) -> Result<LambdaReferences, &'static str> {
        let view = self.context.view();
        let mut pending = vec![HirTreeChild::Expr(expr)];
        let mut references = LambdaReferences::default();
        while let Some(node) = pending.pop() {
            match node {
                HirTreeChild::Expr(expr) => {
                    let node = view
                        .expr(expr)
                        .ok_or("lambda capture expression is missing")?;
                    if let HirExpr::Path(path) = node.data() {
                        let symbol = match &path.resolution {
                            ResolveResult::Resolved(symbol) => Some(*symbol),
                            ResolveResult::PartiallyResolved(partial) => partial.resolved_prefix,
                            ResolveResult::Unresolved | ResolveResult::Ambiguous(_) => None,
                        };
                        if let Some(symbol) = symbol {
                            references.symbols.insert(symbol);
                        }
                    }
                    // Expression children deliberately stop at latent bodies.
                    // Capture discovery crosses those boundaries without executing them.
                    match node.data() {
                        HirExpr::Field { .. } => {
                            references.value_paths.insert(expr);
                        }
                        HirExpr::Path(path)
                            if matches!(path.resolution, ResolveResult::PartiallyResolved(_)) =>
                        {
                            references.value_paths.insert(expr);
                        }
                        HirExpr::Record(record) => {
                            for field in &record.fields {
                                if let etas_hir::HirFieldInit::Shorthand {
                                    resolution: ResolveResult::Resolved(symbol),
                                    ..
                                } = field
                                {
                                    references.symbols.insert(*symbol);
                                }
                            }
                        }
                        HirExpr::Lambda {
                            body: HirLambdaBody::Expr(expr),
                            ..
                        } => pending.push(HirTreeChild::Expr(*expr)),
                        HirExpr::Lambda {
                            body: HirLambdaBody::Block(block),
                            ..
                        } => pending.push(HirTreeChild::Block(*block)),
                        HirExpr::Match { arms, .. } => {
                            for arm in arms {
                                pending.push(match arm.body {
                                    etas_hir::HirMatchArmBody::Expr(expr) => {
                                        HirTreeChild::Expr(expr)
                                    }
                                    etas_hir::HirMatchArmBody::Block(block) => {
                                        HirTreeChild::Block(block)
                                    }
                                });
                            }
                        }
                        HirExpr::Handler { handlers, .. } => {
                            pending.extend(handlers.iter().copied().map(HirTreeChild::HandlerArm))
                        }
                        _ => {}
                    }
                    pending.extend(node.children());
                }
                HirTreeChild::Stmt(stmt) => {
                    let node = view
                        .stmt(stmt)
                        .ok_or("lambda capture statement is missing")?;
                    pending.extend(node.children());
                }
                HirTreeChild::Block(block) => {
                    let node = view.block(block).ok_or("lambda capture block is missing")?;
                    pending.extend(node.statements().map(|stmt| HirTreeChild::Stmt(stmt.id())));
                    if let Some(expr) = node.final_expr() {
                        pending.push(HirTreeChild::Expr(expr.id()));
                    }
                }
                HirTreeChild::HandlerArm(arm) => {
                    let node = view
                        .handler_arm(arm)
                        .ok_or("lambda capture handler arm is missing")?;
                    pending.push(HirTreeChild::Block(node.data().body));
                }
                HirTreeChild::Pattern(_) => {}
            }
        }
        Ok(references)
    }
}
