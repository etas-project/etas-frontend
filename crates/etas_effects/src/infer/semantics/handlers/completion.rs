use super::*;
use etas_hir::{HirArg, ResolveResult, SymbolDef};

impl EffectSemantics<'_> {
    // This value boundary has one total checked call and only constant finish
    // alternatives. No resumptions, earlier control exits or arm actions are
    // hidden by collapsing it into a normal expression result.
    pub(super) fn checked_value_handler_completion(
        &self,
        expr: HirExprId,
        handler: HirExprId,
        entry: &EffectState,
        body_state: &EffectState,
        (handled, produced): (&EffectRow, &EffectRow),
    ) -> bool {
        if !entry.summary.escaping_effects.is_empty()
            || body_state.incomplete
            || !produced.is_empty()
        {
            return false;
        }
        let Some(HirExpr::Handle { body, .. }) = self.hir.exprs.get(expr) else {
            return false;
        };
        let call = match self.hir.exprs.get(*body) {
            Some(HirExpr::Block(block)) => {
                let Some(block) = self.hir.blocks.get(*block) else {
                    return false;
                };
                if !block.stmts.is_empty() {
                    return false;
                }
                let Some(expr) = block.final_expr else {
                    return false;
                };
                expr
            }
            _ => *body,
        };
        let Some(HirExpr::Call { args, .. }) = self.hir.exprs.get(call) else {
            return false;
        };
        if self.checked_intrinsic_completion(call)
            != Some(etas_std::IntrinsicCompletionSemantics::ReturnsOrRaisesDeclaredErrors)
            || !args.iter().all(|arg| {
                self.immediate_handler_value(match arg {
                    HirArg::Positional(value) | HirArg::Named { value, .. } => *value,
                })
            })
        {
            return false;
        }
        let escaping = &body_state.summary.escaping_effects;
        if escaping.is_empty()
            || escaping.open.is_some()
            || !escaping
                .effects
                .iter()
                .all(|effect| matches!(effect, crate::Effect::Error(_)))
        {
            return false;
        }
        let coverage = EffectCoverage {
            registry: self.registry,
            types: &self.types.store,
        };
        if !coverage.row_covers(handled, escaping) {
            return false;
        }
        let Some(HirExpr::Handler { handlers, .. }) = self.hir.exprs.get(handler) else {
            return false;
        };
        !handlers.is_empty()
            && handlers.iter().all(|arm| {
                let Some(arm) = self.hir.handler_arms.get(*arm) else {
                    return false;
                };
                if !arm.patterns.iter().all(|pat| {
                    matches!(
                        self.hir.pats.get(*pat),
                        Some(etas_hir::HirPat::Binding { .. } | etas_hir::HirPat::Wildcard { .. })
                    )
                }) {
                    return false;
                }
                let Some(block) = self.hir.blocks.get(arm.body) else {
                    return false;
                };
                if block.final_expr.is_some() || block.stmts.len() != 1 {
                    return false;
                }
                match self.hir.stmts.get(block.stmts[0]) {
                    Some(HirStmt::Finish { value, .. }) => self.immediate_handler_value(*value),
                    _ => false,
                }
            })
    }

    fn immediate_handler_value(&self, expr: HirExprId) -> bool {
        match self.hir.exprs.get(expr) {
            Some(HirExpr::Literal(_)) => self.types.facts.expr_types.contains_key(&expr),
            Some(HirExpr::Tuple { elems, .. }) if elems.is_empty() => {
                self.types.facts.expr_types.contains_key(&expr)
            }
            Some(HirExpr::Path(path)) => {
                let ResolveResult::Resolved(symbol) = path.resolution else {
                    return false;
                };
                let Some(symbol) = self.type_symbols.canonical_symbol(self.hir, symbol) else {
                    return false;
                };
                match self.hir.symbols.get(symbol).map(|symbol| &symbol.def) {
                    Some(
                        SymbolDef::Param { .. }
                        | SymbolDef::Local { .. }
                        | SymbolDef::PatternBinding { .. },
                    ) => true,
                    Some(SymbolDef::Item { item }) => matches!(
                        self.hir.items.get(*item),
                        Some(etas_hir::HirItem::Agent(_) | etas_hir::HirItem::Flow(_))
                    ),
                    _ => false,
                }
            }
            _ => false,
        }
    }
}
