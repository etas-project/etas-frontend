pub mod collect {
    pub mod aliases;
    pub mod callables;
    pub mod effect_actions;
    pub mod imports;
    pub mod nominal_types;
    pub mod resource_handles;
    pub mod specs;
    pub mod std_actions;
}
pub mod imports;
pub mod materialize;
pub mod solve {
    pub mod aliases;
    pub mod spec_closure;
}
pub mod state;
pub mod validate {
    pub mod annotations;
    pub mod exported_contracts;
    pub mod flow_spec_satisfaction;
    pub mod spec_impls;
}

use crate::pipeline::context::TypePipelineContext;

pub fn run_for_hir(ctx: &mut TypePipelineContext<'_>) {
    imports::std::apply_hir_std_prelude(ctx);
    run(ctx);
}

pub fn run_from_input(
    ctx: &mut TypePipelineContext<'_>,
    input: &crate::SignaturePipelineInput<'_>,
) {
    imports::std::apply_hir_std_prelude(ctx);
    imports::std::apply_std_signature_input(ctx, &input.std);
    imports::external::apply_external_signature_input(ctx, &input.external);
    run(ctx);
    imports::source::apply_source_signature_input(ctx, &input.source);
}

pub fn run(ctx: &mut TypePipelineContext<'_>) {
    let mut state = state::SignaturePipelineState::default();
    collect::imports::collect_imports(ctx, &mut state);
    collect::nominal_types::collect_nominal_types(ctx, &mut state);
    publish_signature_prefix(ctx, &state);
    collect::aliases::collect_aliases(ctx, &mut state);
    publish_signature_prefix(ctx, &state);
    collect::specs::collect_specs(ctx, &mut state);
    collect::specs::record_type_param_bounds(ctx, &mut state);
    publish_signature_prefix(ctx, &state);
    collect::nominal_types::collect_members(ctx, &mut state);
    publish_signature_prefix(ctx, &state);
    collect::effect_actions::collect_effect_actions(ctx, &mut state);
    collect::std_actions::collect_std_actions(ctx, &mut state);
    publish_action_signature_prefix(ctx, &state);
    collect::callables::collect_callables(ctx, &mut state);
    solve::aliases::solve_aliases(ctx, &mut state);
    solve::spec_closure::solve_spec_closure(ctx, &mut state);
    validate::annotations::validate_annotations(ctx, &state);
    validate::spec_impls::validate_spec_impls(ctx, &state);
    validate::flow_spec_satisfaction::validate_flow_spec_satisfaction(ctx, &mut state);
    validate::exported_contracts::validate_exported_contracts(ctx, &state);
    materialize::materialize_signature_facts(ctx, state);
}

fn publish_action_signature_prefix(
    ctx: &mut TypePipelineContext<'_>,
    state: &state::SignaturePipelineState,
) {
    ctx.signature_facts.action_signatures.extend(
        state
            .action_signatures
            .iter()
            .map(|(symbol, signature)| (*symbol, signature.clone())),
    );
    ctx.signature_facts.qualified_action_signatures.extend(
        state
            .qualified_action_signatures
            .iter()
            .map(|(name, signature)| (name.clone(), signature.clone())),
    );
    ctx.signature_facts
        .symbol_types
        .extend(
            state
                .symbol_types
                .iter()
                .filter_map(|(symbol, fact)| match fact {
                    crate::SymbolTypeFact::EffectAction { .. } => Some((*symbol, fact.clone())),
                    _ => None,
                }),
        );
}

fn publish_signature_prefix(
    ctx: &mut TypePipelineContext<'_>,
    state: &state::SignaturePipelineState,
) {
    ctx.signature_facts.symbol_types.extend(
        state
            .symbol_types
            .iter()
            .map(|(symbol, fact)| (*symbol, fact.clone())),
    );
    ctx.signature_facts.type_param_bounds.extend(
        state
            .type_param_bounds
            .iter()
            .map(|(symbol, facts)| (*symbol, facts.clone())),
    );
    ctx.signature_facts.spec_signatures.extend(
        state
            .spec_signatures
            .iter()
            .map(|(symbol, fact)| (*symbol, fact.clone())),
    );
}
