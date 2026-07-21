use crate::{
    PrimitiveType, TypeId,
    pipeline::{
        body::collect::{expr::collect_expr, stmt::collect_stmt},
        context::BodyCollectContext,
    },
};

pub fn collect_block(
    ctx: &mut BodyCollectContext<'_, '_>,
    block: etas_hir::HirBlockId,
    expected: Option<TypeId>,
) -> TypeId {
    let block_data = ctx.ctx.hir.blocks[block].clone();
    let mut last_stmt = None;
    for stmt in block_data.stmts {
        last_stmt = Some(collect_stmt(ctx, stmt));
    }
    block_data
        .final_expr
        .map(|expr| collect_expr(ctx, expr, expected))
        .unwrap_or_else(|| last_stmt.unwrap_or_else(|| ctx.primitive(PrimitiveType::Unit)))
}
