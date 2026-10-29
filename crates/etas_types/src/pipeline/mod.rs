pub mod body;
pub mod context;
pub mod finalize;
pub mod signature;
pub mod symbols;

use etas_hir::HirProgram;

pub use context::TypePipelineContext;

pub fn check_project(hir: &HirProgram) -> crate::TypeOutput {
    let mut ctx = TypePipelineContext::new(hir);
    signature::run_for_hir(&mut ctx);
    body::output_inference::run(&mut ctx);
    let items = hir.items.iter().map(|(id, _)| id).collect::<Vec<_>>();
    for item in items {
        body::run(&mut ctx, item);
    }
    ctx.finish()
}

pub fn check_signatures(hir: &HirProgram) -> crate::TypeOutput {
    let mut ctx = TypePipelineContext::new(hir);
    signature::run_for_hir(&mut ctx);
    ctx.finish()
}

pub fn run_signature_pipeline(input: crate::SignaturePipelineInput<'_>) -> crate::TypeOutput {
    let mut ctx =
        TypePipelineContext::new_with_std_registry(input.program, input.std_registry.clone());
    signature::run_from_input(&mut ctx, &input);
    ctx.finish()
}

pub fn check_top_level_items(
    hir: &HirProgram,
    seed: crate::TypeOutput,
    std_registry: std::sync::Arc<etas_std::StdRegistry>,
) -> crate::TypeOutput {
    let mut ctx = TypePipelineContext {
        hir,
        std_registry,
        interner: crate::TypeInterner::from_store(std::sync::Arc::unwrap_or_clone(seed.store)),
        signature_facts: std::sync::Arc::unwrap_or_clone(seed.facts),
        symbols: std::sync::Arc::new(symbols::TypeSymbolIndex::build(hir)),
        signature_base: None,
        body_parameters: None,
        diagnostics: Vec::new(),
        lowering_type_stack: Vec::new(),
        type_applications: Vec::new(),
        external_type_paths: std::collections::HashMap::new(),
    };
    for (item, data) in hir.items.iter() {
        if matches!(data, etas_hir::HirItem::TopLevelLet(_)) {
            body::run(&mut ctx, item);
        }
    }
    ctx.finish()
}

pub fn check_body_delta(
    hir: &HirProgram,
    base: &crate::SignatureEnvironment,
    item: etas_hir::HirItemId,
    std_registry: std::sync::Arc<etas_std::StdRegistry>,
) -> crate::BodyTypeOutput {
    let mut ctx = TypePipelineContext::for_body(hir, base, std_registry);
    body::run(&mut ctx, item);
    body_delta_output(ctx, base, item)
}

pub fn check_body_delta_controlled(
    hir: &HirProgram,
    base: &crate::SignatureEnvironment,
    item: etas_hir::HirItemId,
    std_registry: std::sync::Arc<etas_std::StdRegistry>,
    control: &dyn etas_utils::PipelineExecutionControl,
) -> Result<crate::BodyTypeOutput, etas_utils::PipelineInterruption> {
    let mut ctx = TypePipelineContext::for_body(hir, base, std_registry);
    body::run_controlled(&mut ctx, item, control)?;
    Ok(body_delta_output(ctx, base, item))
}

fn body_delta_output(
    ctx: TypePipelineContext<'_>,
    base: &crate::SignatureEnvironment,
    item: etas_hir::HirItemId,
) -> crate::BodyTypeOutput {
    crate::BodyTypeOutput {
        owner: item,
        base: base.types.identity(),
        base_len: base.types.len(),
        types: ctx.interner.into_store().into_local_types(),
        facts: ctx.signature_facts,
        diagnostics: ctx.diagnostics,
    }
}

pub fn check_body(request: crate::CheckBodyRequest<'_>) -> crate::TypeOutput {
    let mut ctx = TypePipelineContext::new(request.hir);
    signature::run_for_hir(&mut ctx);
    body::output_inference::run(&mut ctx);
    body::run(&mut ctx, request.item);
    ctx.finish()
}

pub fn infer_callable_outputs(
    hir: &HirProgram,
    seed: crate::TypeOutput,
    std_registry: std::sync::Arc<etas_std::StdRegistry>,
) -> crate::TypeOutput {
    infer_outputs_with_checkpoint(hir, seed, std_registry, &|| {
        Ok::<(), std::convert::Infallible>(())
    })
    .unwrap_or_else(|never| match never {})
}

pub fn infer_callable_outputs_controlled(
    hir: &HirProgram,
    seed: crate::TypeOutput,
    std_registry: std::sync::Arc<etas_std::StdRegistry>,
    control: &dyn etas_utils::PipelineExecutionControl,
) -> Result<crate::TypeOutput, etas_utils::PipelineInterruption> {
    infer_outputs_with_checkpoint(hir, seed, std_registry, &|| control.checkpoint())
}

fn infer_outputs_with_checkpoint<E>(
    hir: &HirProgram,
    seed: crate::TypeOutput,
    std_registry: std::sync::Arc<etas_std::StdRegistry>,
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<crate::TypeOutput, E> {
    checkpoint()?;
    let mut ctx = TypePipelineContext {
        hir,
        std_registry,
        interner: crate::TypeInterner::from_store(std::sync::Arc::unwrap_or_clone(seed.store)),
        signature_facts: std::sync::Arc::unwrap_or_clone(seed.facts),
        symbols: std::sync::Arc::new(symbols::TypeSymbolIndex::build(hir)),
        signature_base: None,
        body_parameters: None,
        diagnostics: seed.diagnostics,
        lowering_type_stack: Vec::new(),
        type_applications: Vec::new(),
        external_type_paths: std::collections::HashMap::new(),
    };
    body::output_inference::run_with_checkpoint(&mut ctx, checkpoint)?;
    checkpoint()?;
    Ok(ctx.finish())
}
