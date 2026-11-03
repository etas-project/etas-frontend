use std::collections::HashMap;

use etas_hir::HirExprId;

use crate::{EffectFacts, EffectPipelineError, EffectRow, effect_var_id_from_name};

pub(super) fn solved_contextual_rows(
    types: &etas_types::TypeOutput,
    facts: &EffectFacts,
) -> Result<HashMap<HirExprId, HashMap<String, EffectRow>>, EffectPipelineError> {
    let mut sources = HashMap::<_, HashMap<_, _>>::new();
    for (call, instantiation) in &types.facts.generic_instantiations {
        if instantiation.deferred_effect_row_obligations.is_empty() {
            continue;
        }
        let invalid = || EffectPipelineError::InvalidCallableFacts {
            reason: format!("deferred callable row has no unique solved binding at {call:?}"),
        };
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
