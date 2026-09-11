mod oracle;
#[cfg(test)]
mod tests;

use etas_hir::{HirExprId, HirItem, HirProgram, SymbolDef};
use etas_hir_analysis::{
    alias::*,
    unit::{HirAnonymousFlowBody, HirSemanticUnit},
};
use etas_types::{EffectArgRef, Type, TypeOutput};

/// Checked resource origins; unknown alias information is never an empty authority set.
pub(crate) struct MemoryProvenance {
    facts: AliasFacts,
    converged: bool,
}

impl MemoryProvenance {
    pub(crate) fn analyze(
        hir: &HirProgram,
        types: &TypeOutput,
        registry: &etas_std::StdRegistry,
        units: &[crate::EffectUnit],
    ) -> Self {
        let analysis = analyze_aliases(AliasAnalysisInput {
            hir,
            units: units
                .iter()
                .filter_map(|unit| semantic_unit(hir, *unit))
                .collect(),
            config: AliasPrecisionConfig {
                field_sensitivity: FieldSensitivity::FullProjection,
                heap_model: HeapModel::ResourceAware,
                // Projection depth is bounded by source structure, not a magic nesting limit.
                max_projection_depth: hir.exprs.len(),
                ..AliasPrecisionConfig::balanced()
            },
            oracle: oracle::MemoryOracle::new(hir, types, registry),
        });
        let converged = analysis
            .convergence
            .iter()
            .all(|component| component.status == etas_utils::ConvergenceStatus::Converged);
        Self {
            facts: analysis.facts,
            converged,
        }
    }

    pub(crate) fn arguments(
        &self,
        hir: &HirProgram,
        types: &TypeOutput,
        expr: HirExprId,
    ) -> Result<Vec<EffectArgRef>, String> {
        if !self.converged {
            return Err("memory provenance analysis did not converge".into());
        }
        // Direct roots/projections are already proven by type checking.
        if let Some(ty) = types.facts.expr_memory_places.get(&expr) {
            let Some(Type::MemoryPlace(place)) = types.store.get(*ty) else {
                return Err("checked memory place fact has the wrong type".into());
            };
            return Ok(vec![EffectArgRef::Path(place.segments.clone())]);
        }
        let value = self
            .facts
            .expr_aliases
            .get(&expr)
            .ok_or_else(|| format!("missing resource provenance for {expr:?}"))?;
        let AliasSet::Known(places) = &value.may else {
            return Err(format!("resource provenance for {expr:?} is unresolved"));
        };
        if places.is_empty() {
            return Err(format!(
                "resource provenance for {expr:?} has no checked origin"
            ));
        }
        let mut arguments = Vec::new();
        for place in places {
            let mut path = match &place.root {
                AliasTarget::MemoryPlace(path) => path.clone(),
                AliasTarget::Param { unit, index } => {
                    let HirSemanticUnit::Item(item) = unit else {
                        return Err("resource parameter has no callable signature".into());
                    };
                    let params = match hir.items.get(*item) {
                        Some(HirItem::Flow(flow)) => &flow.params,
                        Some(HirItem::Tool(tool)) => &tool.params,
                        Some(HirItem::Agent(agent)) => &agent.params,
                        _ => return Err("resource parameter owner is not callable".into()),
                    };
                    let symbol = params
                        .get(*index)
                        .and_then(|symbol| hir.symbols.get(*symbol))
                        .ok_or_else(|| "resource parameter is missing".to_owned())?;
                    if !matches!(symbol.def, SymbolDef::Param { .. }) {
                        return Err("resource parameter has the wrong symbol kind".into());
                    }
                    vec![symbol.name.clone()]
                }
                _ => {
                    return Err(format!(
                        "resource origin is not a checked Store: {:?}",
                        place.root
                    ));
                }
            };
            for projection in &place.projections {
                match projection {
                    Projection::Field(field) => path.push(field.clone()),
                    _ => return Err("resource projection is not a statically named Store".into()),
                }
            }
            let arg = EffectArgRef::Path(path);
            if !arguments.contains(&arg) {
                arguments.push(arg);
            }
        }
        Ok(arguments)
    }
}

fn semantic_unit(hir: &HirProgram, unit: crate::EffectUnit) -> Option<HirSemanticUnit> {
    use crate::infer::unit::{EffectAnonymousFlowBody, EffectUnit};
    Some(match unit {
        EffectUnit::Item(item) => match hir.items.get(item) {
            Some(HirItem::TopLevelLet(_)) => HirSemanticUnit::TopLevelLet(item),
            _ => HirSemanticUnit::Item(item),
        },
        EffectUnit::HandlerArm { arm, .. } => HirSemanticUnit::HandlerArm(arm),
        EffectUnit::AnonymousFlow { owner, value, body } => HirSemanticUnit::AnonymousFlow {
            owner,
            expr: value,
            body: match body {
                EffectAnonymousFlowBody::Expr(expr) => HirAnonymousFlowBody::Expr(expr),
                EffectAnonymousFlowBody::Block(block) => HirAnonymousFlowBody::Block(block),
            },
        },
        // Call sites are not declarations; they are visited inside their owning bodies.
        EffectUnit::FirstClassFlowCall { .. } => return None,
    })
}
