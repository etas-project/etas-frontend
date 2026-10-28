use super::{collect::entry::collect_body_item, state::BodyPipelineState};
use crate::{ItemSignature, PrimitiveType, Type, pipeline::context::TypePipelineContext};
use std::sync::Arc;

/// Inferred declaration outputs form one constraint graph, so forward and
/// mutually recursive calls share output variables before body deltas freeze.
pub(crate) fn run(ctx: &mut TypePipelineContext<'_>) {
    run_with_checkpoint(ctx, &|| Ok::<(), std::convert::Infallible>(()))
        .unwrap_or_else(|never| match never {})
}

pub(crate) fn run_with_checkpoint<E>(
    ctx: &mut TypePipelineContext<'_>,
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<(), E> {
    checkpoint()?;
    let inferred = ctx
        .hir
        .items
        .iter()
        .filter_map(|(id, item)| {
            matches!(item, etas_hir::HirItem::Flow(flow) if flow.return_type.is_none())
                .then_some(id)
        })
        .collect::<Vec<_>>();
    let Some(first) = inferred.first().copied() else {
        return Ok(());
    };
    let facts = Arc::new(std::mem::take(&mut ctx.signature_facts));
    let base = crate::SignatureEnvironment {
        types: crate::ty::SharedTypeBase::new(std::mem::take(&mut ctx.interner).into_store()),
        specs: Arc::new(crate::SpecFacts::from(facts.as_ref())),
        facts,
        symbols: ctx.symbols.clone(),
        diagnostics: Arc::from([]),
    };
    let mut inference = TypePipelineContext::for_body(ctx.hir, &base, ctx.std_registry.clone());
    let mut aggregate = BodyPipelineState::new(first);
    for item in &inferred {
        checkpoint()?;
        let mut state = BodyPipelineState::new(*item);
        collect_body_item(&mut inference, &mut state, checkpoint)?;
        aggregate.constraints.extend(state.constraints);
        aggregate.spec_obligations.extend(state.spec_obligations);
        aggregate.validations.extend(state.validations);
        aggregate
            .provisional
            .expr_types
            .extend(state.provisional.expr_types);
        aggregate
            .provisional
            .stmt_types
            .extend(state.provisional.stmt_types);
        aggregate
            .provisional
            .pat_types
            .extend(state.provisional.pat_types);
    }
    inference.interner.primitive(PrimitiveType::I32);
    inference.interner.primitive(PrimitiveType::F64);
    super::solve::run(
        &mut aggregate,
        &mut inference.interner,
        &base.specs,
        &base.facts,
        ctx.hir.items[first].span(),
        checkpoint,
    )?;
    checkpoint()?;
    super::validate::run(&mut inference, &aggregate);
    ctx.diagnostics.append(&mut inference.diagnostics);
    let mut source = inference.interner;
    let base_len = base.types.len();
    ctx.interner = crate::TypeInterner::from_store(crate::TypeStore::with_base(base.types));
    ctx.signature_facts = Arc::unwrap_or_clone(base.facts);
    if !aggregate.solver_report.failures.is_empty() {
        return Ok(());
    }
    if let Err(error) = crate::pipeline::finalize::remap::substitute_facts(
        &mut ctx.signature_facts,
        &mut source,
        &aggregate.solver_report.substitutions,
    )
    .and_then(|()| {
        crate::pipeline::finalize::remap::project_signature_facts(
            &mut ctx.signature_facts,
            source.store(),
            &mut ctx.interner,
            base_len,
        )
    }) {
        ctx.diagnostics.push(etas_core::Diagnostic::type_check(
            etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
            ctx.hir.items[first].span(),
            format!("inferred callable outputs could not be materialized: {error}"),
        ));
        return Ok(());
    }
    crate::pipeline::signature::validate::flow_spec_satisfaction::validate_materialized_callable_signatures(ctx);
    for item in inferred {
        checkpoint()?;
        let Some(ItemSignature::Flow(signature)) = ctx.signature_facts.item_signatures.get(&item)
        else {
            ctx.diagnostics.push(etas_core::Diagnostic::type_check(
                etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                ctx.hir.items[item].span(),
                "inferred flow is missing its declaration signature",
            ));
            continue;
        };
        if matches!(
            ctx.interner.store().get(signature.output),
            Some(Type::Var(_))
        ) {
            ctx.diagnostics.push(etas_core::Diagnostic::type_check(
                etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                ctx.hir.items[item].span(),
                "recursive callable output is unconstrained; add a return type annotation",
            ));
        }
    }
    Ok(())
}
