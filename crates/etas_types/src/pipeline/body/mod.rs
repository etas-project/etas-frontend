pub mod collect {
    pub mod action_selector;
    pub mod agent_method;
    pub mod block;
    pub mod call;
    pub mod callable_instantiation;
    pub mod callable_reference;
    pub mod entry;
    pub mod enum_variant;
    pub mod expr;
    pub mod field;
    pub mod handler;
    pub(in crate::pipeline) mod handler_request;
    pub mod index;
    pub mod inference;
    pub mod lambda;
    pub mod literal;
    pub mod loop_stmt;
    pub mod method_call;
    pub mod nominal_constructor;
    pub mod pattern;
    pub mod perform;
    pub mod record;
    pub mod reflection;
    pub mod spec_method_call;
    pub mod std_member;
    mod std_type_member;
    pub mod stmt;
}

pub mod materialize;
pub mod output_inference;
pub mod solve;
pub mod state;
pub mod validate;

#[cfg(test)]
mod execution_tests;

use crate::pipeline::context::TypePipelineContext;

use self::{collect::entry::collect_body_item, state::BodyPipelineState};

pub fn run(ctx: &mut TypePipelineContext<'_>, item: etas_hir::HirItemId) {
    run_with_checkpoint(ctx, item, &|| Ok::<(), std::convert::Infallible>(()))
        .unwrap_or_else(|never| match never {})
}

pub fn run_controlled(
    ctx: &mut TypePipelineContext<'_>,
    item: etas_hir::HirItemId,
    control: &dyn etas_utils::PipelineExecutionControl,
) -> Result<(), etas_utils::PipelineInterruption> {
    run_with_checkpoint(ctx, item, &|| control.checkpoint())
}

fn run_with_checkpoint<E>(
    ctx: &mut TypePipelineContext<'_>,
    item: etas_hir::HirItemId,
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<(), E> {
    checkpoint()?;
    let mut state = BodyPipelineState::new(item);
    collect_body_item(ctx, &mut state, checkpoint)?;
    let (obligations, validations) = crate::lower::type_application::take_obligations(ctx);
    state.spec_obligations.extend(obligations);
    state.validations.extend(validations);
    checkpoint()?;
    ctx.interner.primitive(crate::PrimitiveType::I32);
    ctx.interner.primitive(crate::PrimitiveType::F64);
    let spec_facts = ctx
        .signature_base
        .map(|base| base.specs.clone())
        .unwrap_or_else(|| std::sync::Arc::new(crate::SpecFacts::from(&ctx.signature_facts)));
    let declarations = ctx
        .signature_base
        .map_or(&ctx.signature_facts, |base| &base.facts);
    solve::run(
        &mut state,
        &mut ctx.interner,
        &spec_facts,
        declarations,
        ctx.hir.items[item].span(),
        checkpoint,
    )?;
    checkpoint()?;
    validate::run(ctx, &state);
    checkpoint()?;
    materialize::run(ctx, state);
    Ok(())
}
