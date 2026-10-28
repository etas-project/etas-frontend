use crate::{
    PrimitiveType, TypeId,
    pipeline::{
        body::collect::{expr::collect_expr, stmt::collect_stmt},
        context::BodyCollectContext,
    },
};

pub fn collect_block<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    block: etas_hir::HirBlockId,
    expected: Option<TypeId>,
) -> Result<TypeId, E> {
    ctx.checkpoint()?;
    let block_data = ctx.ctx.hir.blocks[block].clone();
    let mut last_stmt = None;
    for stmt in block_data.stmts {
        ctx.checkpoint()?;
        last_stmt = Some(collect_stmt(ctx, stmt)?);
    }
    let ty = block_data
        .final_expr
        .map(|expr| collect_expr(ctx, expr, expected))
        .transpose()?
        .unwrap_or_else(|| last_stmt.unwrap_or_else(|| ctx.primitive(PrimitiveType::Unit)));
    Ok(ty)
}
