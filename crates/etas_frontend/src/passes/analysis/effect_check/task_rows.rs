use crate::{ProjectContext, project::instantiation};

pub(super) fn refine(context: &mut ProjectContext) -> Result<bool, String> {
    let hir = &context.hir.as_ref().expect("checked HIR").hir;
    let types = context.types.as_mut().expect("checked types");
    let effects = context.effects.as_ref().expect("solved effects");
    let registry = &context
        .effect_pipeline_artifacts
        .as_ref()
        .expect("effect artifacts")
        .registry;
    let mut calls = effects.facts.generic_effect_rows.iter().collect::<Vec<_>>();
    calls.sort_by_key(|(call, _)| **call);
    let mut solved = Vec::new();
    for (call, rows) in calls {
        if !types
            .facts
            .expr_types
            .get(call)
            .and_then(|ty| {
                etas_types::source_execution::task(&types.store, &types.facts.known_std_types, *ty)
            })
            .is_some_and(|task| task.effects.tail.is_some())
        {
            continue;
        }
        let owner = *hir
            .index()
            .expr_item
            .get(call)
            .ok_or("task result has no checked expression owner")?;
        let bindings = rows
            .iter()
            .filter(|(_, row)| row.open.is_none())
            .map(|(name, row)| {
                Ok((
                    name.clone(),
                    instantiation::checked_row(
                        row,
                        registry,
                        instantiation::signature(&types.facts, owner),
                        std::iter::empty(),
                    )?,
                ))
            })
            .collect::<Result<_, String>>()?;
        solved.push(etas_types::ResolvedEffectRowInstantiation {
            call: *call,
            bindings,
        });
    }
    etas_types::pipeline::finalize::task_rows::refine_task_value_rows(hir, types, &solved)
}
