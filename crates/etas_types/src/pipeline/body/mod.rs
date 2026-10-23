pub mod collect {
    pub mod action_selector;
    pub mod block;
    pub mod call;
    pub mod entry;
    pub mod enum_variant;
    pub mod expr;
    pub mod field;
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
    let spec_facts = ctx
        .signature_base
        .map(|base| base.specs.clone())
        .unwrap_or_else(|| std::sync::Arc::new(crate::SpecFacts::from(&ctx.signature_facts)));
    let known = &ctx
        .signature_base
        .map_or(&ctx.signature_facts, |base| &base.facts)
        .known_std_types;
    solve::run(
        &mut state,
        &mut ctx.interner,
        &spec_facts,
        known,
        ctx.hir.items[item].span(),
    );
    validate::run(ctx, &state);
    materialize::run(ctx, state);
}
