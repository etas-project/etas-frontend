use etas_core::{Diagnostic, EffectDiagnosticCode};
use etas_hir::{HirItem, HirItemId, SymbolId};
use etas_types::Type;

use crate::{
    Effect, EffectAnalysisOutput, EffectFacts, EffectOutput, EffectPipelineError, EffectRegistry,
    EffectRow, EffectSet, EffectUnit, LatentEffectBodyRef, LatentEffectFact,
    effect_var_id_from_name,
};

mod contextual_rows;

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
            handled_body_requests: analysis.inputs.handled_body_requests,
            frozen_callable_graphs: analysis.inputs.frozen_callable_graphs,
            callable_references: analysis.inputs.callable_references,
            metadata_callables: analysis.inputs.metadata_callables,
            metadata_callable_graphs: analysis.inputs.metadata_callable_graphs,
            captured_callables: analysis.inputs.captured_callables,
            native_call_targets: analysis.inputs.native_call_targets,
            generic_effect_rows: analysis.inputs.generic_effect_rows,
            callable_environments: analysis.inputs.callable_environments,
            method_evidence: analysis.inputs.method_evidence,
            inferred_parameter_effects: analysis.inputs.inferred_parameter_effects,
            handler_action_effects: analysis.inputs.handler_action_effects,
            expr_effects: analysis.inputs.expr_effects,
            stmt_effects: analysis.inputs.stmt_effects,
            unit_effects,
            handler_values: analysis.inputs.handler_values,
            handle_applications: analysis.inputs.handle_applications,
            performed_actions: analysis.inputs.performed_actions,
            try_captures: analysis.inputs.try_captures,
            latent_effects: analysis.inputs.latent_effects,
            std_callable_values: analysis.inputs.std_callable_values,
            std_action_calls: analysis.inputs.std_action_calls,
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
            let summary_item = if let HirItem::Agent(agent) = hir_item
                && !matches!(agent.body, etas_hir::HirAgentBody::Source { .. })
            {
                let Some(etas_types::ItemSignature::Agent(signature)) =
                    types.facts.item_signatures.get(&item)
                else {
                    diagnostics.push(Diagnostic::effect_check(
                        EffectDiagnosticCode::IncompleteEffectFacts,
                        agent.span,
                        "agent component is missing checked signature facts",
                    ));
                    continue;
                };
                if signature.default_entry.is_none() {
                    continue;
                }
                let Some(method) = types.facts.agent_default_method(item) else {
                    diagnostics.push(Diagnostic::effect_check(
                        EffectDiagnosticCode::IncompleteEffectFacts,
                        agent.span,
                        "agent default invocation is missing checked method identity",
                    ));
                    continue;
                };
                method.body
            } else {
                item
            };
            let Some(summary) = facts
                .unit_effects
                .get(&EffectUnit::Item(summary_item))
                .cloned()
            else {
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
    let contextual_rows = if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error)
    {
        // Failed analysis retains its diagnostics, but cannot publish checked facts.
        std::collections::HashMap::new()
    } else {
        contextual_rows::solved_contextual_rows(types, facts)?
    };
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
        let mut declared_bound = match types.store.get(flow_type) {
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
        if let Some(bound) = &mut declared_bound
            && let Some(bindings) = contextual_rows.get(&value)
        {
            contextual_rows::specialize_bound(bound, bindings);
        }
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
