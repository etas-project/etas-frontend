use crate::pipeline::{context::TypePipelineContext, signature::state::SignaturePipelineState};

pub fn materialize_signature_facts(
    ctx: &mut TypePipelineContext<'_>,
    mut state: SignaturePipelineState,
) {
    ctx.signature_facts
        .enum_layouts
        .extend(state.enum_layouts.drain());
    ctx.signature_facts
        .symbol_types
        .extend(state.symbol_types.drain());
    ctx.signature_facts
        .resource_handles
        .extend(state.resource_handles.drain());
    ctx.signature_facts
        .item_signatures
        .extend(state.item_signatures.drain());
    ctx.signature_facts
        .action_signatures
        .extend(state.action_signatures.drain());
    ctx.signature_facts
        .qualified_action_signatures
        .extend(state.qualified_action_signatures.drain());
    ctx.signature_facts
        .spec_signatures
        .extend(state.spec_signatures.drain());
    ctx.signature_facts.spec_impls.extend(state.spec_impls);
    ctx.signature_facts
        .type_spec_satisfactions
        .extend(state.type_spec_satisfactions);
    ctx.signature_facts
        .callable_spec_satisfactions
        .extend(state.callable_spec_satisfactions);
    ctx.signature_facts
        .trace_spec_conformances
        .extend(state.trace_spec_conformances);
    ctx.signature_facts
        .type_param_bounds
        .extend(state.type_param_bounds.drain());
}
