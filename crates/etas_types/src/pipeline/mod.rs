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
        diagnostics: Vec::new(),
        lowering_type_stack: Vec::new(),
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
    body::run(&mut ctx, request.item);
    ctx.finish()
}
