mod external;
mod source;
mod standard;

use crate::pipeline::{context::TypePipelineContext, signature::state::SignaturePipelineState};

pub fn collect(ctx: &mut TypePipelineContext<'_>, state: &mut SignaturePipelineState) {
    state.methods.extend(source::collect(ctx, state));
    state.methods.extend(standard::collect(ctx));
    state.methods.extend(external::collect(ctx));
}
