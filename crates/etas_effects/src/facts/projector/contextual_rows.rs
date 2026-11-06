use std::collections::HashMap;

use etas_hir::HirExprId;

use crate::{
    EffectFacts, EffectPipelineError, EffectRegistry, EffectRow, EffectSet, EffectUnit,
    effect_var_id_from_name,
};

pub(super) fn solved_contextual_rows(
    types: &etas_types::TypeOutput,
    registry: &EffectRegistry,
    facts: &mut EffectFacts,
) -> Result<HashMap<HirExprId, HashMap<String, EffectRow>>, EffectPipelineError> {
    let mut sources = HashMap::<_, HashMap<_, _>>::new();
    for (call, instantiation) in &types.facts.generic_instantiations {
        if instantiation.deferred_effect_row_obligations.is_empty() {
            continue;
        }
        let invalid = || EffectPipelineError::InvalidCallableFacts {
            reason: format!("deferred callable row has no unique solved binding at {call:?}"),
        };
        if !facts.generic_effect_rows.contains_key(call) {
            let rows = solve_from_latent_units(
                registry,
                facts,
                &instantiation.deferred_effect_row_obligations,
            )
            .ok_or_else(invalid)?;
            facts.generic_effect_rows.insert(*call, rows);
        }
        let rows = facts.generic_effect_rows.get(call).ok_or_else(invalid)?;
        for obligation in &instantiation.deferred_effect_row_obligations {
            let mut matches = rows.iter().filter(|(name, _)| name == &obligation.param);
            let (_, row) = matches.next().ok_or_else(invalid)?;
            if matches.next().is_some() {
                return Err(invalid());
            }
            if sources
                .entry(obligation.source)
                .or_default()
                .insert(obligation.param.clone(), row.clone())
                .is_some_and(|previous| previous != *row)
            {
                return Err(invalid());
            }
        }
    }
    Ok(sources)
}

fn solve_from_latent_units(
    registry: &EffectRegistry,
    facts: &EffectFacts,
    obligations: &[etas_types::DeferredEffectRowObligation],
) -> Option<Vec<(String, EffectRow)>> {
    let mut solved = std::collections::BTreeMap::<String, EffectRow>::new();
    for obligation in obligations {
        let actual = facts.unit_effects.iter().find_map(|(unit, summary)| {
            matches!(unit, EffectUnit::AnonymousFlow { value, .. } if *value == obligation.source)
                .then_some(&summary.escaping_effects)
        })?;
        let fixed = super::row_from_type_ref(registry, &obligation.fixed);
        if fixed.open.is_some()
            || fixed
                .effects
                .iter()
                .any(|effect| !actual.effects.contains(effect))
        {
            return None;
        }
        let binding = EffectRow {
            effects: EffectSet::from_iter(
                actual
                    .effects
                    .iter()
                    .filter(|effect| !fixed.effects.contains(effect))
                    .cloned(),
            ),
            open: actual.open,
        };
        if solved
            .insert(obligation.param.clone(), binding.clone())
            .is_some_and(|previous| previous != binding)
        {
            return None;
        }
    }
    Some(solved.into_iter().collect())
}

pub(super) fn specialize_bound(row: &mut EffectRow, bindings: &HashMap<String, EffectRow>) {
    if let Some(open) = row.open
        && let Some((_, binding)) = bindings
            .iter()
            .find(|(name, _)| effect_var_id_from_name(name) == open)
    {
        row.effects.union_assign(&binding.effects);
        row.open = binding.open;
    }
}
