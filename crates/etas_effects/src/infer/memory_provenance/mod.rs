mod oracle;
#[cfg(test)]
mod tests;

use etas_hir::{HirExprId, HirItem, HirProgram, SymbolDef};
use etas_hir_analysis::{
    alias::*,
    unit::{HirAnonymousFlowBody, HirSemanticUnit},
};
use etas_types::{EffectArgRef, Type, TypeOutput};

/// Analyze aliases on the first request for a resource origin. All solver
/// rounds in this pipeline use the same checked program and selected units.
pub(crate) struct MemoryProvenanceQuery<'a> {
    types: &'a TypeOutput,
    registry: &'a etas_std::StdRegistry,
    units: Vec<crate::EffectUnit>,
    context: etas_hir_analysis::HirAnalysisContext,
    origins: std::sync::OnceLock<MemoryProvenance>,
}

impl<'a> MemoryProvenanceQuery<'a> {
    pub(crate) fn new(
        types: &'a TypeOutput,
        registry: &'a etas_std::StdRegistry,
        units: &[crate::EffectUnit],
        context: &etas_hir_analysis::HirAnalysisContext,
    ) -> Self {
        Self {
            types,
            registry,
            units: units.to_vec(),
            context: context.clone(),
            origins: Default::default(),
        }
    }

    pub(crate) fn arguments(&self, expr: HirExprId) -> Result<Vec<EffectArgRef>, String> {
        self.origins()
            .arguments(self.context.program(), self.types, expr)
    }

    fn origins(&self) -> &MemoryProvenance {
        self.origins.get_or_init(|| {
            MemoryProvenance::analyze(self.types, self.registry, &self.units, &self.context)
        })
    }

    /// Distinct or unknown session origins share the conservative wildcard.
    pub(crate) fn pty_profile(&self, expr: HirExprId) -> Option<EffectArgRef> {
        let origins = self.origins();
        if !origins.converged {
            return None;
        }
        let place = origins.facts.expr_aliases.get(&expr)?.must.as_ref()?;
        let AliasTarget::Symbol(symbol) = place.root else {
            return None;
        };
        if !place.projections.is_empty() {
            return None;
        }
        let hir = self.context.program();
        let path = super::pty::static_profile_path(hir, self.registry, symbol)?;
        Some(EffectArgRef::Path(path))
    }
}

/// Checked resource origins; unknown alias information is never an empty authority set.
pub(crate) struct MemoryProvenance {
    facts: AliasFacts,
    converged: bool,
}

impl MemoryProvenance {
    pub(crate) fn analyze(
        types: &TypeOutput,
        registry: &etas_std::StdRegistry,
        units: &[crate::EffectUnit],
        context: &etas_hir_analysis::HirAnalysisContext,
    ) -> Self {
        let hir = context.revision();
        let analysis = analyze_aliases(AliasAnalysisInput {
            hir,
            units: units.iter().map(|unit| semantic_unit(hir, *unit)).collect(),
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

fn semantic_unit(hir: &HirProgram, unit: crate::EffectUnit) -> HirSemanticUnit {
    use crate::infer::unit::{EffectAnonymousFlowBody, EffectUnit};
    match unit {
        EffectUnit::ParameterDefault {
            owner,
            parameter,
            expr,
        } => HirSemanticUnit::ParameterDefault {
            owner,
            parameter,
            expr,
        },
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
    }
}
