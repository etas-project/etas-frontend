pub mod collect {
    pub mod action_selector;
    pub mod block;
    pub mod call;
    pub mod entry;
    pub mod expr;
    pub mod handler;
    pub mod index;
    pub mod lambda;
    pub mod literal;
    pub mod loop_stmt;
    pub mod method_call;
    pub mod pattern;
    pub mod perform;
    pub mod record;
    pub mod spec_method_call;
    pub mod std_member;
    pub mod stmt;
}

pub mod materialize;
pub mod solve;
pub mod state;
pub mod validate;

use crate::pipeline::context::TypePipelineContext;

use self::{collect::entry::collect_body_item, state::BodyPipelineState};

pub fn run(ctx: &mut TypePipelineContext<'_>, item: etas_hir::HirItemId) {
    let mut state = BodyPipelineState::new(item);
    collect_body_item(ctx, &mut state);
    ctx.interner.primitive(crate::PrimitiveType::I32);
    ctx.interner.primitive(crate::PrimitiveType::F64);
    let spec_facts = crate::SpecFacts {
        signatures: ctx.signature_facts.spec_signatures.clone(),
        impls: ctx.signature_facts.spec_impls.clone(),
        type_satisfactions: ctx.signature_facts.type_spec_satisfactions.clone(),
        callable_satisfactions: ctx.signature_facts.callable_spec_satisfactions.clone(),
        trace_conformances: ctx.signature_facts.trace_spec_conformances.clone(),
        external_callable_satisfactions: ctx
            .signature_facts
            .external_callable_spec_satisfactions
            .clone(),
        external_trace_conformances: ctx.signature_facts.external_trace_spec_conformances.clone(),
        type_param_bounds: ctx.signature_facts.type_param_bounds.clone(),
    };
    solve::run(&mut state, ctx.interner.store(), &spec_facts);
    validate::run(ctx, &state);
    materialize::run(ctx, state);
}
