use super::program::LowerCtx;
use crate::*;
use etas_core::Span;
use etas_syntax::{ast, ast::*};

impl LowerCtx {
    pub(super) fn lower_block(
        &mut self,
        block: &ast::Block,
        parent_scope: ScopeId,
        item_id: HirItemId,
    ) -> HirBlockId {
        let block_id = self.hir.blocks.alloc(HirBlock {
            id: HirBlockId(0),
            stmts: Vec::new(),
            final_expr: None,
            scope: ScopeId(0),
            span: block.span,
        });
        let scope = self
            .scopes
            .alloc(Some(parent_scope), ScopeOwner::Block(block_id), block.span);
        let lowered = self.lower_block_contents(block, block_id, scope, item_id);
        *self.hir.blocks.get_mut(block_id).unwrap() = lowered;
        block_id
    }

    pub(super) fn lower_block_contents(
        &mut self,
        block: &ast::Block,
        block_id: HirBlockId,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirBlock {
        let stmts = block
            .stmts
            .iter()
            .map(|stmt| self.lower_stmt(stmt, scope, item_id))
            .collect();
        let final_expr = block
            .final_expr
            .as_ref()
            .map(|expr| self.lower_expr(expr, scope, item_id));
        self.source_map.map_block(block_id, block.span);
        HirBlock {
            id: block_id,
            stmts,
            final_expr,
            scope,
            span: block.span,
        }
    }

    pub(super) fn lower_stmt(
        &mut self,
        stmt: &Stmt,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirStmtId {
        let span = stmt_span(stmt);
        if let Stmt::Let(stmt) = stmt {
            let id = self.hir.stmts.alloc(HirStmt::Error { span: stmt.span });
            let value = self.lower_expr(&stmt.value, scope, item_id);
            let type_annotation = stmt
                .ty
                .as_ref()
                .map(|ty| self.lower_type(ty, scope, item_id));
            self.local_binding = Some(super::program::LocalBindingContext {
                binding: id,
                ty: type_annotation,
                initializer: Some(value),
            });
            let pat = self.lower_pattern(&stmt.pattern, scope, item_id);
            self.local_binding = None;
            *self.hir.stmts.get_mut(id).unwrap() = HirStmt::Let {
                value,
                type_annotation,
                pat,
                span: stmt.span,
            };
            self.map_stmt(id, span);
            return id;
        }
        if let Stmt::Var(stmt) = stmt {
            let id = self.hir.stmts.alloc(HirStmt::Error { span: stmt.span });
            let value = self.lower_expr(&stmt.value, scope, item_id);
            let type_annotation = stmt
                .ty
                .as_ref()
                .map(|ty| self.lower_type(ty, scope, item_id));
            self.local_binding = Some(super::program::LocalBindingContext {
                binding: id,
                ty: type_annotation,
                initializer: Some(value),
            });
            let pat = self.lower_pattern(&stmt.pattern, scope, item_id);
            self.local_binding = None;
            *self.hir.stmts.get_mut(id).unwrap() = HirStmt::Var {
                value,
                type_annotation,
                pat,
                span: stmt.span,
            };
            self.map_stmt(id, span);
            return id;
        }
        if let Stmt::For(stmt) = stmt {
            let id = self.hir.stmts.alloc(HirStmt::Error { span: stmt.span });
            let iter = self.lower_expr(&stmt.iter, scope, item_id);
            let limits = stmt
                .limits
                .iter()
                .map(|limit| self.lower_expr(limit, scope, item_id))
                .collect();
            let body = self.hir.blocks.alloc(HirBlock {
                id: HirBlockId(0),
                stmts: Vec::new(),
                final_expr: None,
                scope: ScopeId(0),
                span: stmt.body.span,
            });
            let body_scope =
                self.scopes
                    .alloc(Some(scope), ScopeOwner::Block(body), stmt.body.span);
            self.pattern_binding = Some(super::program::PatternBindingContext {
                owner: PatternBindingOwner::For { stmt: id },
                ty: None,
                initializer: Some(iter),
            });
            let pat = self.lower_pattern(&stmt.pattern, body_scope, item_id);
            self.pattern_binding = None;
            let lowered = self.lower_block_contents(&stmt.body, body, body_scope, item_id);
            *self.hir.blocks.get_mut(body).unwrap() = lowered;
            *self.hir.stmts.get_mut(id).unwrap() = HirStmt::For {
                pat,
                iter,
                limits,
                body,
                span: stmt.span,
            };
            self.map_stmt(id, span);
            return id;
        }
        let hir_stmt = match stmt {
            Stmt::Let(_) | Stmt::Var(_) | Stmt::For(_) => {
                unreachable!("binding statements return early")
            }
            Stmt::Assign(stmt) => HirStmt::Assign {
                target: self.lower_expr(&stmt.target, scope, item_id),
                value: self.lower_expr(&stmt.value, scope, item_id),
                span: stmt.span,
            },
            Stmt::If(stmt) => HirStmt::If(self.lower_if_stmt_expr(stmt, scope, item_id)),
            Stmt::Match(stmt) => HirStmt::Match(self.lower_expr(
                &Expr::Match(ast::MatchExpr {
                    scrutinee: Box::new(stmt.scrutinee.clone()),
                    arms: stmt.arms.clone(),
                    span: stmt.span,
                }),
                scope,
                item_id,
            )),
            Stmt::While(stmt) => HirStmt::While {
                cond: self.lower_expr(&stmt.condition, scope, item_id),
                limits: stmt
                    .limits
                    .iter()
                    .map(|limit| self.lower_expr(limit, scope, item_id))
                    .collect(),
                body: self.lower_block(&stmt.body, scope, item_id),
                span: stmt.span,
            },
            Stmt::Retry(stmt) => HirStmt::Retry {
                limits: stmt
                    .limits
                    .iter()
                    .map(|limit| self.lower_expr(limit, scope, item_id))
                    .collect(),
                body: self.lower_block(&stmt.body, scope, item_id),
                span: stmt.span,
            },
            Stmt::Resume(stmt) => HirStmt::Resume {
                value: stmt
                    .value
                    .as_ref()
                    .map(|value| self.lower_expr(value, scope, item_id)),
                span: stmt.span,
            },
            Stmt::Finish(stmt) => HirStmt::Finish {
                value: self.lower_expr(&stmt.value, scope, item_id),
                span: stmt.span,
            },
            Stmt::Return(stmt) => HirStmt::Return {
                value: stmt
                    .value
                    .as_ref()
                    .map(|value| self.lower_expr(value, scope, item_id)),
                span: stmt.span,
            },
            Stmt::Break(span) => HirStmt::Break { span: *span },
            Stmt::Continue(span) => HirStmt::Continue { span: *span },
            Stmt::Expr(stmt) => HirStmt::Expr {
                expr: self.lower_expr(&stmt.expr, scope, item_id),
                span: stmt.span,
            },
            Stmt::Error(span) => HirStmt::Error { span: *span },
        };
        let id = self.hir.stmts.alloc(hir_stmt);
        self.map_stmt(id, span);
        id
    }

    fn map_stmt(&mut self, id: HirStmtId, span: Span) {
        self.source_map.map_stmt(id, span);
    }

    fn lower_if_stmt_expr(
        &mut self,
        stmt: &IfStmt,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirExprId {
        let cond = self.lower_expr(&stmt.condition, scope, item_id);
        let then_block = self.lower_block(&stmt.then_branch, scope, item_id);
        let else_branch = match &stmt.else_branch {
            Some(ElseBranch::If(stmt)) => Some(HirElseBranch::If(
                self.lower_if_stmt_expr(stmt, scope, item_id),
            )),
            Some(ElseBranch::Block(block)) => Some(HirElseBranch::Block(
                self.lower_block(block, scope, item_id),
            )),
            None => None,
        };
        self.alloc_expr(
            HirExpr::If {
                cond,
                then_block,
                else_branch,
                span: stmt.span,
            },
            stmt.span,
        )
    }
}

fn stmt_span(stmt: &Stmt) -> Span {
    match stmt {
        Stmt::Let(stmt) => stmt.span,
        Stmt::Var(stmt) => stmt.span,
        Stmt::Assign(stmt) => stmt.span,
        Stmt::If(stmt) => stmt.span,
        Stmt::Match(stmt) => stmt.span,
        Stmt::For(stmt) => stmt.span,
        Stmt::While(stmt) => stmt.span,
        Stmt::Retry(stmt) => stmt.span,
        Stmt::Resume(stmt) => stmt.span,
        Stmt::Finish(stmt) => stmt.span,
        Stmt::Return(stmt) => stmt.span,
        Stmt::Break(span) | Stmt::Continue(span) | Stmt::Error(span) => *span,
        Stmt::Expr(stmt) => stmt.span,
    }
}
