use std::collections::HashMap;

use etas_hir::{HirItem, HirItemId, HirRevision};

use crate::{EffectFacts, EffectOutput, EffectUnit};

/// Item-owned facts, including its nested lambdas and handler arms. Registry,
/// project diagnostics and entry selection remain on the project revision.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, etas_core::RetainedSize)]
pub struct BodyEffectOutput {
    pub owner: HirItemId,
    pub facts: EffectFacts,
}

impl BodyEffectOutput {
    pub fn project(
        output: &EffectOutput,
        hir: &HirRevision,
        items: impl IntoIterator<Item = HirItemId>,
    ) -> Result<HashMap<HirItemId, Self>, String> {
        let index = hir.index();
        let mut bodies = HashMap::new();
        let mut symbols = HashMap::new();
        for item in items {
            let declaration = hir
                .items
                .get(item)
                .ok_or_else(|| format!("effect body references missing HIR item {item:?}"))?;
            let symbol = match declaration {
                HirItem::Flow(flow) => Some(flow.symbol),
                HirItem::Tool(tool) => Some(tool.symbol),
                HirItem::Agent(agent) => Some(agent.symbol),
                HirItem::TopLevelLet(value) => Some(value.symbol),
                _ => None,
            };
            if let Some(symbol) = symbol {
                symbols.insert(symbol, item);
            }
            bodies.insert(
                item,
                Self {
                    owner: item,
                    facts: EffectFacts::default(),
                },
            );
        }

        // Visit each project fact once and construct compact local maps directly.
        // A cloned global map followed by retain would keep global capacity.
        macro_rules! expressions {
            ($($field:ident),+ $(,)?) => {$(
                for (expr, fact) in &output.facts.$field {
                    let owner = index.expr_item.get(expr).ok_or_else(|| {
                        format!("effect fact has no validated expression owner: {expr:?}")
                    })?;
                    if let Some(body) = bodies.get_mut(owner) {
                        body.facts.$field.insert(*expr, fact.clone());
                    }
                }
            )+};
        }
        expressions!(
            handled_body_requests,
            native_call_targets,
            generic_effect_rows,
            expr_effects,
            handler_values,
            handle_applications,
            performed_actions,
            try_captures,
            latent_effects,
            std_callable_values,
            std_action_calls
        );
        for (stmt, fact) in &output.facts.stmt_effects {
            let owner = index
                .stmt_item
                .get(stmt)
                .ok_or_else(|| format!("effect fact has no validated statement owner: {stmt:?}"))?;
            if let Some(body) = bodies.get_mut(owner) {
                body.facts.stmt_effects.insert(*stmt, fact.clone());
            }
        }
        for (symbol, summary) in &output.facts.inferred_parameter_effects {
            let Some(etas_hir::SymbolDef::Param { owner, .. }) =
                hir.symbols.get(*symbol).map(|data| &data.def)
            else {
                return Err(format!(
                    "inferred callback behavior has no parameter owner: {symbol:?}"
                ));
            };
            if let Some(body) = bodies.get_mut(owner) {
                body.facts
                    .inferred_parameter_effects
                    .insert(*symbol, summary.clone());
            }
        }
        for (id, snapshot) in &output.facts.callable_environments {
            if let Some(body) = bodies.get_mut(&snapshot.owner) {
                body.facts
                    .callable_environments
                    .insert(*id, snapshot.clone());
            }
        }
        for (key, proof) in &output.facts.frozen_callable_graphs {
            if let Some(body) = bodies.get_mut(&proof.owner) {
                body.facts
                    .frozen_callable_graphs
                    .insert(*key, proof.clone());
            }
        }
        for (id, analysis) in &output.facts.captured_callables {
            let owner = output
                .facts
                .callable_environments
                .get(&id.environment)
                .ok_or_else(|| format!("captured callable has no checked environment: {id:?}"))?
                .owner;
            if let Some(body) = bodies.get_mut(&owner) {
                body.facts.captured_callables.insert(*id, analysis.clone());
            }
        }
        for (source, analysis) in &output.facts.metadata_callables {
            let Some(environment) = source.environment else {
                continue;
            };
            let owner = output
                .facts
                .callable_environments
                .get(&environment)
                .ok_or_else(|| format!("metadata callable has no checked environment: {source:?}"))?
                .owner;
            if let Some(body) = bodies.get_mut(&owner) {
                body.facts
                    .metadata_callables
                    .insert(*source, analysis.clone());
            }
        }
        for (reference, analysis) in &output.facts.callable_references {
            let owner = match reference.environment {
                Some(environment) => {
                    output
                        .facts
                        .callable_environments
                        .get(&environment)
                        .ok_or_else(|| {
                            format!("callable reference has no checked environment: {reference:?}")
                        })?
                        .owner
                }
                None => reference.owner,
            };
            if let Some(body) = bodies.get_mut(&owner) {
                body.facts
                    .callable_references
                    .insert(reference.clone(), analysis.clone());
            }
        }
        for (unit, fact) in &output.facts.unit_effects {
            let owner = match unit {
                EffectUnit::Item(item) => item,
                EffectUnit::HandlerArm { owner, .. } | EffectUnit::AnonymousFlow { owner, .. } => {
                    owner
                }
            };
            if let Some(body) = bodies.get_mut(owner) {
                body.facts.unit_effects.insert(*unit, fact.clone());
            }
        }
        for contract in &output.facts.public_contracts {
            if let Some(body) = bodies.get_mut(&contract.item) {
                body.facts.public_contracts.push(contract.clone());
            }
        }
        for (item, body) in &mut bodies {
            if let Some(summary) = output.facts.item_effects.get(item) {
                body.facts.item_effects.insert(*item, summary.clone());
            }
            if let Some(requirements) = output.facts.requirements.items.get(item) {
                body.facts
                    .requirements
                    .items
                    .insert(*item, requirements.clone());
            }
            if let Some(specs) = output.facts.trace_specs.items.get(item) {
                body.facts.trace_specs.items.insert(*item, specs.clone());
            }
            if let Some(models) = output.facts.trace_specs.conformances.get(item) {
                body.facts
                    .trace_specs
                    .conformances
                    .insert(*item, models.clone());
            }
            if let Some(support) = output.facts.interpreter_support.items.get(item) {
                body.facts
                    .interpreter_support
                    .items
                    .insert(*item, support.clone());
            }
        }
        for (symbol, item) in symbols {
            let body = bodies.get_mut(&item).ok_or_else(|| {
                format!("effect symbol {symbol:?} has no projected body {item:?}")
            })?;
            if let Some(summary) = output.facts.symbol_effects.get(&symbol) {
                body.facts.symbol_effects.insert(symbol, summary.clone());
            }
            if let Some(requirements) = output.facts.requirements.symbols.get(&symbol) {
                body.facts
                    .requirements
                    .symbols
                    .insert(symbol, requirements.clone());
            }
            if let Some(specs) = output.facts.trace_specs.symbols.get(&symbol) {
                body.facts.trace_specs.symbols.insert(symbol, specs.clone());
            }
        }
        Ok(bodies)
    }
}
