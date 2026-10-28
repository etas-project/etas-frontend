use etas_hir::{HirExpr, HirExprId, HirLambdaBody, HirTreeChild, ResolveResult, SymbolId};
use std::collections::BTreeSet;

use super::{
    engine::EffectSemantics,
    state::{EffectState, LatentEnv},
};

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
        }
        Ok(captures)
    }

    pub(super) fn lambda_referenced_symbols(
        &self,
        expr: HirExprId,
    ) -> Result<BTreeSet<SymbolId>, &'static str> {
        if let Some(symbols) = self.lambda_capture_symbols.borrow().get(&expr) {
            return Ok(symbols.clone());
        }
        let symbols = self.collect_lambda_references(expr)?;
        self.lambda_capture_symbols
            .borrow_mut()
            .insert(expr, symbols.clone());
        Ok(symbols)
    }

    fn collect_lambda_references(
        &self,
        expr: HirExprId,
    ) -> Result<BTreeSet<SymbolId>, &'static str> {
        let view = self.context.view();
        let mut pending = vec![HirTreeChild::Expr(expr)];
        let mut symbols = BTreeSet::new();
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
                            symbols.insert(symbol);
                        }
                    }
                    // Expression children deliberately stop at latent bodies.
                    // Capture discovery crosses those boundaries without executing them.
                    match node.data() {
                        HirExpr::Record(record) => {
                            for field in &record.fields {
                                if let etas_hir::HirFieldInit::Shorthand {
                                    resolution: ResolveResult::Resolved(symbol),
                                    ..
                                } = field
                                {
                                    symbols.insert(*symbol);
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
        Ok(symbols)
    }
}
