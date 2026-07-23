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

pub fn check_body_with_seed(
    hir: &HirProgram,
    seed: crate::TypeOutput,
    item: etas_hir::HirItemId,
) -> crate::TypeOutput {
    check_body_with_seed_and_std_registry(
        hir,
        seed,
        item,
        std::sync::Arc::new(etas_std::standard_registry()),
    )
}

pub fn check_body_with_seed_and_std_registry(
    hir: &HirProgram,
    seed: crate::TypeOutput,
    item: etas_hir::HirItemId,
    std_registry: std::sync::Arc<etas_std::StdRegistry>,
) -> crate::TypeOutput {
    let mut ctx = TypePipelineContext {
        hir,
        std_registry,
        interner: crate::TypeInterner::from_store(seed.store),
        signature_facts: seed.facts,
        symbols: symbols::TypeSymbolIndex::build(hir),
        diagnostics: Vec::new(),
        lowering_type_stack: Vec::new(),
        external_type_paths: std::collections::HashMap::new(),
    };
    body::run(&mut ctx, item);
    ctx.finish()
}

pub fn check_body(request: crate::CheckBodyRequest<'_>) -> crate::TypeOutput {
    let mut ctx = TypePipelineContext::new(request.hir);
    signature::run_for_hir(&mut ctx);
    body::run(&mut ctx, request.item);
    ctx.finish()
}

pub fn finalize_type_outputs(
    signatures: crate::TypeOutput,
    body_outputs: Vec<crate::TypeOutput>,
) -> crate::TypeOutput {
    finalize::run(signatures, body_outputs)
}
