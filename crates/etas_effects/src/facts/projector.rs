use etas_core::{Diagnostic, EffectDiagnosticCode};
use etas_hir::{HirItem, HirItemId, SymbolId};
use etas_types::Type;

use crate::{
    Effect, EffectAnalysisOutput, EffectFacts, EffectOutput, EffectPipelineError, EffectRegistry,
    EffectRow, EffectSet, EffectUnit, LatentEffectBodyRef, LatentEffectFact,
    effect_var_id_from_name,
};

pub struct EffectFactProjector;

impl EffectFactProjector {
    pub fn materialize(
        hir: &etas_hir::HirProgram,
        types: &etas_types::TypeOutput,
        registry: &EffectRegistry,
        mut analysis: EffectAnalysisOutput,
        reachable_items: Option<&std::collections::BTreeSet<HirItemId>>,
    ) -> Result<EffectOutput, EffectPipelineError> {
        let mut latent_realizations = std::mem::take(&mut analysis.inputs.latent_realizations);
        let unit_effects = analysis
            .summaries
            .iter()
            .map(|(unit, summary)| (*unit, summary.clone()))
            .chain(analysis.inputs.unit_effects)
            .collect();
        let mut facts = EffectFacts {
            expr_effects: analysis.inputs.expr_effects,
            stmt_effects: analysis.inputs.stmt_effects,
            unit_effects,
            handler_values: analysis.inputs.handler_values,
            handle_applications: analysis.inputs.handle_applications,
            performed_actions: analysis.inputs.performed_actions,
            try_captures: analysis.inputs.try_captures,
            latent_effects: analysis.inputs.latent_effects,
            ..EffectFacts::default()
        };
        let mut diagnostics = analysis.diagnostics;

        materialize_latent_effects(
            hir,
            types,
            registry,
            &mut facts,
            &mut diagnostics,
            &mut latent_realizations,
        )?;

        for (item, hir_item) in hir.items.iter() {
            if reachable_items.is_some_and(|reachable| !reachable.contains(&item)) {
                continue;
            }
            if !requires_materialized_item_summary(hir_item) {
                continue;
            }
            let Some(summary) = facts.unit_effects.get(&EffectUnit::Item(item)).cloned() else {
                diagnostics.push(Diagnostic::effect_check(
                    EffectDiagnosticCode::IncompleteEffectFacts,
                    hir_item.span(),
                    "effect fact materialization requires a solved item summary",
                ));
                continue;
            };
            let Some(symbol) = item_symbol(hir_item) else {
                diagnostics.push(Diagnostic::effect_check(
                    EffectDiagnosticCode::IncompleteEffectFacts,
                    hir_item.span(),
                    "effect fact materialization requires a materialized item symbol",
                ));
                continue;
            };
            facts.item_effects.insert(item, summary.clone());
            facts.symbol_effects.insert(symbol, summary.clone());
            facts
                .requirements
                .items
                .insert(item, summary.requirements.clone());
            facts
                .requirements
                .symbols
                .insert(symbol, summary.requirements.clone());
            facts
                .interpreter_support
                .items
                .insert(item, summary.support.clone());
            if symbol_name(hir, symbol) == Some("main") {
                facts.interpreter_support.entry = Some(summary.support);
            }
        }

        Ok(EffectOutput { facts, diagnostics })
    }
}

fn materialize_latent_effects(
    hir: &etas_hir::HirProgram,
    types: &etas_types::TypeOutput,
    registry: &EffectRegistry,
    facts: &mut EffectFacts,
    diagnostics: &mut Vec<Diagnostic>,
    latent_realizations: &mut std::collections::HashMap<
        etas_hir::HirExprId,
        Vec<etas_hir::HirExprId>,
    >,
) -> Result<(), EffectPipelineError> {
    for (unit, summary) in facts.unit_effects.clone() {
        let EffectUnit::AnonymousFlow { value, body, .. } = unit else {
            continue;
        };
        let Some(flow_type) = types.facts.expr_types.get(&value).copied() else {
            let span = hir
                .exprs
                .get(value)
                .map(|expr| expr.span(&hir.blocks))
                .ok_or(EffectPipelineError::MissingExprFact { expr: value })?;
            diagnostics.push(Diagnostic::effect_check(
                EffectDiagnosticCode::IncompleteEffectFacts,
                span,
                "latent flow fact materialization requires a checked function type fact",
            ));
            continue;
        };
        let declared_bound = match types.store.get(flow_type) {
            Some(Type::Function(flow)) => flow
                .effects
                .as_ref()
                .map(|row| row_from_type_ref(registry, row)),
            _ => {
                let span = hir
                    .exprs
                    .get(value)
                    .map(|expr| expr.span(&hir.blocks))
                    .ok_or(EffectPipelineError::MissingExprFact { expr: value })?;
                diagnostics.push(Diagnostic::effect_check(
                    EffectDiagnosticCode::IncompleteEffectFacts,
                    span,
                    "latent flow fact materialization requires a checked function type",
                ));
                continue;
            }
        };
        let body = match body {
            crate::infer::unit::EffectAnonymousFlowBody::Expr(expr) => {
                LatentEffectBodyRef::Expr(expr)
            }
            crate::infer::unit::EffectAnonymousFlowBody::Block(block) => {
                LatentEffectBodyRef::Block(block)
            }
        };
        let realized_at = latent_realizations.remove(&value).unwrap_or_default();
        facts.latent_effects.insert(
            value,
            LatentEffectFact {
                value,
                body,
                flow_type,
                inferred: summary.escaping_effects.clone(),
                declared_bound,
                summary,
                realized_at,
            },
        );
    }
    Ok(())
}

fn row_from_type_ref(registry: &EffectRegistry, row: &etas_types::EffectRowRef) -> EffectRow {
    EffectRow {
        effects: EffectSet::from_iter(
            row.effects
                .iter()
                .filter_map(|effect| effect_from_type_ref(registry, effect)),
        ),
        open: row.tail.as_deref().map(effect_var_id_from_name),
    }
}

fn effect_from_type_ref(
    registry: &EffectRegistry,
    effect: &etas_types::EffectRef,
) -> Option<Effect> {
    if is_core_error_effect_name(registry, &effect.name) {
        if let Some(etas_types::EffectArgRef::Type(error)) = effect.args.first() {
            return Some(Effect::Error(*error));
        }
    }
    if let Some(action) = registry.action_by_name(&effect.name) {
        return if effect.args.is_empty() {
            Some(Effect::Action(action))
        } else {
            Some(Effect::AppliedAction(crate::ActionInstanceRef {
                action,
                args: effect.args.clone(),
            }))
        };
    }
    registry.tag_by_name(&effect.name).map(|tag| {
        if effect.args.is_empty() {
            Effect::Tag(tag)
        } else {
            Effect::Applied {
                tag,
                args: effect
                    .args
                    .iter()
                    .filter_map(|arg| match arg {
                        etas_types::EffectArgRef::Type(ty) => Some(*ty),
                        _ => None,
                    })
                    .collect(),
            }
        }
    })
}

fn is_core_error_effect_name(registry: &EffectRegistry, name: &str) -> bool {
    name == "Error"
        || registry.tag_by_name(name) == Some(crate::ERROR_TAG)
        || (name.starts_with("std.") && name.rsplit('.').next() == Some("Error"))
}

fn requires_materialized_item_summary(item: &HirItem) -> bool {
    matches!(
        item,
        HirItem::Flow(_) | HirItem::Tool(_) | HirItem::Agent(_) | HirItem::TopLevelLet(_)
    )
}

fn item_symbol(item: &HirItem) -> Option<SymbolId> {
    match item {
        HirItem::Flow(flow) => Some(flow.symbol),
        HirItem::Tool(tool) => Some(tool.symbol),
        HirItem::Agent(agent) => Some(agent.symbol),
        HirItem::TopLevelLet(value) => Some(value.symbol),
        _ => None,
    }
}

fn symbol_name(hir: &etas_hir::HirProgram, symbol: SymbolId) -> Option<&str> {
    hir.symbols.get(symbol).map(|symbol| symbol.name.as_str())
}
