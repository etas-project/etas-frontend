use std::collections::HashSet;

use etas_hir::HirExprId;

use crate::ProjectContext;

pub(super) fn invalidate_replaced_expressions(
    context: &mut ProjectContext,
    expressions: impl IntoIterator<Item = HirExprId>,
) {
    let expressions = expressions.into_iter().collect::<HashSet<_>>();
    let types = context
        .types
        .as_mut()
        .expect("type facts must precede typed-HIR optimization");
    let facts = std::sync::Arc::make_mut(&mut types.facts);
    facts
        .existential_packs
        .retain(|expr, _| !expressions.contains(expr));
    facts
        .checked_method_calls
        .retain(|expr, _| !expressions.contains(expr));
    facts
        .bound_method_values
        .retain(|expr, _| !expressions.contains(expr));
    facts
        .agent_method_calls
        .retain(|expr, _| !expressions.contains(expr));
    facts
        .std_intrinsic_calls
        .retain(|expr, _| !expressions.contains(expr));
    facts
        .action_calls
        .retain(|expr, _| !expressions.contains(expr));
    facts
        .reflections
        .retain(|expr, _| !expressions.contains(expr));
    facts
        .definition_operations
        .retain(|expr, _| !expressions.contains(expr));
    facts
        .agent_inferences
        .retain(|expr, _| !expressions.contains(expr));
    facts
        .expr_memory_places
        .retain(|expr, _| !expressions.contains(expr));
    facts
        .field_projections
        .retain(|expr, _| !expressions.contains(expr));
    facts
        .index_facts
        .retain(|expr, _| !expressions.contains(expr));
    facts
        .slice_facts
        .retain(|expr, _| !expressions.contains(expr));
    facts
        .checked_index_errors
        .retain(|expr, _| !expressions.contains(expr));
    facts
        .generic_instantiations
        .retain(|expr, _| !expressions.contains(expr));
    facts
        .try_facts
        .retain(|expr, _| !expressions.contains(expr));

    let effects = context
        .effects
        .as_mut()
        .expect("effect facts must precede typed-HIR optimization");
    let facts = std::sync::Arc::make_mut(&mut effects.facts);
    facts
        .handled_body_requests
        .retain(|expr, _| !expressions.contains(expr));
    facts
        .native_call_targets
        .retain(|expr, _| !expressions.contains(expr));
    facts
        .metadata_call_targets
        .retain(|expr, _| !expressions.contains(expr));
    facts
        .generic_effect_rows
        .retain(|expr, _| !expressions.contains(expr));
    facts
        .handler_values
        .retain(|expr, _| !expressions.contains(expr));
    facts
        .handle_applications
        .retain(|expr, _| !expressions.contains(expr));
    facts
        .performed_actions
        .retain(|expr, _| !expressions.contains(expr));
    facts
        .try_captures
        .retain(|expr, _| !expressions.contains(expr));
    facts
        .latent_effects
        .retain(|expr, _| !expressions.contains(expr));
    facts
        .std_callable_values
        .retain(|expr, _| !expressions.contains(expr));
    facts
        .std_action_calls
        .retain(|expr, _| !expressions.contains(expr));
}
