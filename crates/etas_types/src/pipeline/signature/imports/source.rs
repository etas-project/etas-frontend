use crate::{SourceSignatureInput, pipeline::context::TypePipelineContext};

pub fn apply_source_signature_input(
    ctx: &mut TypePipelineContext<'_>,
    input: &SourceSignatureInput,
) {
    for binding in &input.symbol_bindings {
        if let Some(fact) = ctx
            .symbols
            .symbol_fact(ctx.hir, &ctx.signature_facts, binding.symbol)
            .cloned()
            .or_else(|| {
                ctx.signature_facts
                    .symbol_types
                    .get(&binding.target)
                    .cloned()
            })
        {
            ctx.signature_facts
                .symbol_types
                .insert(binding.symbol, fact);
        }
    }
}
