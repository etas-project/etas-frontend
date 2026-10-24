use std::collections::HashMap;

use etas_hir::{HirItem, HirItemId, HirProgram, HirTreeIndex};

use crate::{EffectFacts, EffectOutput, EffectUnit};

/// Item-owned facts, including its nested lambdas and handler arms. Registry,
/// project diagnostics and entry selection remain on the project revision.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct BodyEffectOutput {
    pub owner: HirItemId,
    pub facts: EffectFacts,
}

impl BodyEffectOutput {
    pub fn project(
        output: &EffectOutput,
        hir: &HirProgram,
        index: &HirTreeIndex,
        items: impl IntoIterator<Item = HirItemId>,
    ) -> Result<HashMap<HirItemId, Self>, String> {
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
            expr_effects,
            handler_values,
            handle_applications,
            performed_actions,
            try_captures,
            latent_effects
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
        for (unit, fact) in &output.facts.unit_effects {
            let owner = match unit {
                EffectUnit::Item(item) => item,
                EffectUnit::HandlerArm { owner, .. }
                | EffectUnit::AnonymousFlow { owner, .. }
                | EffectUnit::FirstClassFlowCall { owner, .. } => owner,
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
