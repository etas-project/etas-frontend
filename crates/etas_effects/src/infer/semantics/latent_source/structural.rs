use super::{EffectSemantics, EffectState, HirExpr, HirExprId, LatentSource};

impl EffectSemantics<'_> {
    pub(crate) fn record_structural_latent_result(&self, expr: HirExprId, state: &mut EffectState) {
        let mut values = Vec::new();
        let mut blocks = Vec::new();
        match self.hir.exprs.get(expr) {
            Some(HirExpr::Match { arms, .. }) => {
                for arm in arms {
                    match arm.body {
                        etas_hir::HirMatchArmBody::Expr(value) => values.push(value),
                        etas_hir::HirMatchArmBody::Block(block) => blocks.push(block),
                    }
                }
            }
            Some(HirExpr::If {
                then_block,
                else_branch,
                ..
            }) => {
                blocks.push(*then_block);
                match else_branch {
                    Some(etas_hir::HirElseBranch::Block(block)) => blocks.push(*block),
                    Some(etas_hir::HirElseBranch::If(value)) => values.push(*value),
                    None => {}
                }
            }
            Some(HirExpr::Block(block)) => blocks.push(*block),
            _ => return,
        }
        for block in blocks {
            if let Some(value) = self
                .hir
                .blocks
                .get(block)
                .and_then(|block| block.final_expr)
            {
                values.push(value);
            }
        }
        if !self.expr_type_contains_flow(expr)
            && !self.types.facts.expr_types.get(&expr).is_some_and(|ty| {
                values.iter().any(|value| {
                    self.latent_sources_for_expr(state, *value)
                        .is_some_and(|sources| self.symbolic_contract_value(*ty, &sources))
                })
            })
        {
            return;
        }
        let mut sources = std::collections::BTreeSet::new();
        if values.is_empty() {
            sources.insert(LatentSource::Unavailable { expr });
        }
        for value in values {
            if !self.expr_is_never(value) {
                match self.latent_sources_for_expr(state, value) {
                    Some(found) => sources.extend(found),
                    None => {
                        sources.insert(LatentSource::Unavailable { expr: value });
                    }
                }
            }
        }
        state
            .local_latent_values
            .record_value_sources(expr, sources);
    }
}
