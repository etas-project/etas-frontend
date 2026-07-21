use crate::{HirBlockId, HirExprId, HirItemId, HirModuleId};

use super::model::{HirBodyRef, HirTreeChild};
use super::tree::{
    BlockView, BodyView, ExprView, HandlerArmView, HirTreeView, ItemView, ModuleView, PatternView,
    StmtView,
};

pub trait HirVisitor<'view, 'hir> {
    fn enter_module(&mut self, _module: ModuleView<'view, 'hir>) {}
    fn exit_module(&mut self, _module: ModuleView<'view, 'hir>) {}
    fn enter_item(&mut self, _item: ItemView<'view, 'hir>) {}
    fn exit_item(&mut self, _item: ItemView<'view, 'hir>) {}
    fn enter_body(&mut self, _body: BodyView<'view, 'hir>) {}
    fn exit_body(&mut self, _body: BodyView<'view, 'hir>) {}
    fn enter_block(&mut self, _block: BlockView<'view, 'hir>) {}
    fn exit_block(&mut self, _block: BlockView<'view, 'hir>) {}
    fn enter_stmt(&mut self, _stmt: StmtView<'view, 'hir>) {}
    fn exit_stmt(&mut self, _stmt: StmtView<'view, 'hir>) {}
    fn enter_expr(&mut self, _expr: ExprView<'view, 'hir>) {}
    fn exit_expr(&mut self, _expr: ExprView<'view, 'hir>) {}
    fn enter_handler_arm(&mut self, _arm: HandlerArmView<'view, 'hir>) {}
    fn exit_handler_arm(&mut self, _arm: HandlerArmView<'view, 'hir>) {}
    fn enter_pattern(&mut self, _pat: PatternView<'view, 'hir>) {}
    fn exit_pattern(&mut self, _pat: PatternView<'view, 'hir>) {}
}

pub fn walk_module<'view, 'hir, V>(
    view: &'view HirTreeView<'hir>,
    module: HirModuleId,
    visitor: &mut V,
) where
    V: HirVisitor<'view, 'hir>,
{
    walk_from(view, WalkFrame::EnterModule(module), visitor);
}

pub fn walk_item<'view, 'hir, V>(view: &'view HirTreeView<'hir>, item: HirItemId, visitor: &mut V)
where
    V: HirVisitor<'view, 'hir>,
{
    walk_from(view, WalkFrame::EnterItem(item), visitor);
}

pub fn walk_body<'view, 'hir, V>(view: &'view HirTreeView<'hir>, body: HirBodyRef, visitor: &mut V)
where
    V: HirVisitor<'view, 'hir>,
{
    walk_from(view, WalkFrame::EnterBody(body), visitor);
}

pub fn walk_block<'view, 'hir, V>(
    view: &'view HirTreeView<'hir>,
    block: HirBlockId,
    visitor: &mut V,
) where
    V: HirVisitor<'view, 'hir>,
{
    walk_from(view, WalkFrame::EnterBlock(block), visitor);
}

pub fn walk_expr<'view, 'hir, V>(view: &'view HirTreeView<'hir>, expr: HirExprId, visitor: &mut V)
where
    V: HirVisitor<'view, 'hir>,
{
    walk_from(view, WalkFrame::EnterExpr(expr), visitor);
}

#[derive(Clone, Copy)]
enum WalkFrame {
    EnterModule(HirModuleId),
    ExitModule(HirModuleId),
    EnterItem(HirItemId),
    ExitItem(HirItemId),
    EnterBody(HirBodyRef),
    ExitBody(HirBodyRef),
    EnterBlock(HirBlockId),
    ExitBlock(HirBlockId),
    EnterStmt(crate::HirStmtId),
    ExitStmt(crate::HirStmtId),
    EnterExpr(HirExprId),
    ExitExpr(HirExprId),
    EnterHandlerArm(crate::HirHandlerArmId),
    ExitHandlerArm(crate::HirHandlerArmId),
    EnterPattern(crate::HirPatId),
    ExitPattern(crate::HirPatId),
}

fn walk_from<'view, 'hir, V>(view: &'view HirTreeView<'hir>, root: WalkFrame, visitor: &mut V)
where
    V: HirVisitor<'view, 'hir>,
{
    let mut stack = vec![root];
    while let Some(frame) = stack.pop() {
        match frame {
            WalkFrame::EnterModule(module_id) => {
                let module = view
                    .module(module_id)
                    .expect("module id should exist in HIR tree");
                visitor.enter_module(module);
                stack.push(WalkFrame::ExitModule(module_id));
                push_reversed(
                    &mut stack,
                    module.items().map(|item| WalkFrame::EnterItem(item.id())),
                );
            }
            WalkFrame::ExitModule(module_id) => {
                let module = view
                    .module(module_id)
                    .expect("module id should exist in HIR tree");
                visitor.exit_module(module);
            }
            WalkFrame::EnterItem(item_id) => {
                let item = view
                    .item(item_id)
                    .expect("item id should exist in HIR tree");
                visitor.enter_item(item);
                stack.push(WalkFrame::ExitItem(item_id));
                push_reversed(
                    &mut stack,
                    item.bodies().map(|body| WalkFrame::EnterBody(body.id())),
                );
            }
            WalkFrame::ExitItem(item_id) => {
                let item = view
                    .item(item_id)
                    .expect("item id should exist in HIR tree");
                visitor.exit_item(item);
            }
            WalkFrame::EnterBody(body_id) => {
                let body = view
                    .body(body_id)
                    .expect("body id should exist in HIR tree");
                visitor.enter_body(body);
                stack.push(WalkFrame::ExitBody(body_id));
                push_reversed(
                    &mut stack,
                    body.blocks().map(|block| WalkFrame::EnterBlock(block.id())),
                );
                push_reversed(
                    &mut stack,
                    body.root_exprs()
                        .map(|expr| WalkFrame::EnterExpr(expr.id())),
                );
            }
            WalkFrame::ExitBody(body_id) => {
                let body = view
                    .body(body_id)
                    .expect("body id should exist in HIR tree");
                visitor.exit_body(body);
            }
            WalkFrame::EnterBlock(block_id) => {
                let block = view
                    .block(block_id)
                    .expect("block id should exist in HIR tree");
                visitor.enter_block(block);
                stack.push(WalkFrame::ExitBlock(block_id));
                if let Some(expr) = block.final_expr() {
                    stack.push(WalkFrame::EnterExpr(expr.id()));
                }
                push_reversed(
                    &mut stack,
                    block
                        .statements()
                        .map(|stmt| WalkFrame::EnterStmt(stmt.id())),
                );
            }
            WalkFrame::ExitBlock(block_id) => {
                let block = view
                    .block(block_id)
                    .expect("block id should exist in HIR tree");
                visitor.exit_block(block);
            }
            WalkFrame::EnterStmt(stmt_id) => {
                let stmt = view
                    .stmt(stmt_id)
                    .expect("stmt id should exist in HIR tree");
                visitor.enter_stmt(stmt);
                stack.push(WalkFrame::ExitStmt(stmt_id));
                push_reversed(&mut stack, stmt.children().map(child_frame));
            }
            WalkFrame::ExitStmt(stmt_id) => {
                let stmt = view
                    .stmt(stmt_id)
                    .expect("stmt id should exist in HIR tree");
                visitor.exit_stmt(stmt);
            }
            WalkFrame::EnterExpr(expr_id) => {
                let expr = view
                    .expr(expr_id)
                    .expect("expr id should exist in HIR tree");
                visitor.enter_expr(expr);
                stack.push(WalkFrame::ExitExpr(expr_id));
                let start = stack.len();
                expr.for_each_child(|child| stack.push(child_frame(child)));
                stack[start..].reverse();
            }
            WalkFrame::ExitExpr(expr_id) => {
                let expr = view
                    .expr(expr_id)
                    .expect("expr id should exist in HIR tree");
                visitor.exit_expr(expr);
            }
            WalkFrame::EnterHandlerArm(arm_id) => {
                let arm = view
                    .handler_arm(arm_id)
                    .expect("handler arm id should exist in HIR tree");
                visitor.enter_handler_arm(arm);
                stack.push(WalkFrame::ExitHandlerArm(arm_id));
            }
            WalkFrame::ExitHandlerArm(arm_id) => {
                let arm = view
                    .handler_arm(arm_id)
                    .expect("handler arm id should exist in HIR tree");
                visitor.exit_handler_arm(arm);
            }
            WalkFrame::EnterPattern(pat_id) => {
                let pat = view
                    .pattern(pat_id)
                    .expect("pattern id should exist in HIR tree");
                visitor.enter_pattern(pat);
                stack.push(WalkFrame::ExitPattern(pat_id));
                push_reversed(&mut stack, pat.children().map(child_frame));
            }
            WalkFrame::ExitPattern(pat_id) => {
                let pat = view
                    .pattern(pat_id)
                    .expect("pattern id should exist in HIR tree");
                visitor.exit_pattern(pat);
            }
        }
    }
}

fn child_frame(child: HirTreeChild) -> WalkFrame {
    match child {
        HirTreeChild::Expr(expr) => WalkFrame::EnterExpr(expr),
        HirTreeChild::Block(block) => WalkFrame::EnterBlock(block),
        HirTreeChild::Stmt(stmt) => WalkFrame::EnterStmt(stmt),
        HirTreeChild::HandlerArm(arm) => WalkFrame::EnterHandlerArm(arm),
        HirTreeChild::Pattern(pat) => WalkFrame::EnterPattern(pat),
    }
}

fn push_reversed(stack: &mut Vec<WalkFrame>, frames: impl IntoIterator<Item = WalkFrame>) {
    let start = stack.len();
    stack.extend(frames);
    stack[start..].reverse();
}
