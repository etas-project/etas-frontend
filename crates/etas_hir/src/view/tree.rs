use etas_core::Span;

use crate::{
    HirBlock, HirBlockId, HirExpr, HirExprId, HirHandlerArm, HirHandlerArmId, HirItem, HirItemId,
    HirModule, HirModuleId, HirNodeRef, HirPat, HirPatId, HirProgram, HirStmt, HirStmtId, Scope,
    ScopeId,
};

use super::helpers::{
    body_scope, expr_children, for_each_expr_child, for_each_pat_child, for_each_stmt_child,
    item_scope, pat_children, stmt_children, stmt_span,
};
use super::model::{HirBodyRef, HirOwner, HirTreeChild, HirTreeIndex, HirTreeIndexError};

pub struct HirTreeView<'hir> {
    program: &'hir HirProgram,
    index: HirTreeIndex,
}

impl<'hir> HirTreeView<'hir> {
    pub fn try_new(program: &'hir HirProgram) -> Result<Self, HirTreeIndexError> {
        Ok(Self {
            program,
            index: HirTreeIndex::try_build(program)?,
        })
    }

    pub fn from_validated_index(program: &'hir HirProgram, index: HirTreeIndex) -> Self {
        Self { program, index }
    }

    pub fn new(program: &'hir HirProgram) -> Self {
        Self::try_new(program).expect("HIR tree invariants should hold")
    }

    pub fn program(&self) -> &'hir HirProgram {
        self.program
    }

    pub fn index(&self) -> &HirTreeIndex {
        &self.index
    }

    pub fn modules(&self) -> impl Iterator<Item = ModuleView<'_, 'hir>> + '_ {
        self.program
            .modules
            .iter()
            .copied()
            .filter_map(move |id| self.module(id))
    }

    pub fn module(&self, id: HirModuleId) -> Option<ModuleView<'_, 'hir>> {
        self.program
            .modules_arena
            .get(id)
            .map(|_| ModuleView { view: self, id })
    }

    pub fn item(&self, id: HirItemId) -> Option<ItemView<'_, 'hir>> {
        self.program
            .items
            .get(id)
            .map(|_| ItemView { view: self, id })
    }

    pub fn body(&self, id: HirBodyRef) -> Option<BodyView<'_, 'hir>> {
        self.index
            .item_bodies
            .get(&id.item)
            .is_some_and(|bodies| bodies.contains(&id))
            .then_some(BodyView { view: self, id })
    }

    pub fn block(&self, id: HirBlockId) -> Option<BlockView<'_, 'hir>> {
        self.program
            .blocks
            .get(id)
            .map(|_| BlockView { view: self, id })
    }

    pub fn stmt(&self, id: HirStmtId) -> Option<StmtView<'_, 'hir>> {
        self.program
            .stmts
            .get(id)
            .map(|_| StmtView { view: self, id })
    }

    pub fn expr(&self, id: HirExprId) -> Option<ExprView<'_, 'hir>> {
        self.program
            .exprs
            .get(id)
            .map(|_| ExprView { view: self, id })
    }

    pub fn handler_arm(&self, id: HirHandlerArmId) -> Option<HandlerArmView<'_, 'hir>> {
        self.program
            .handler_arms
            .get(id)
            .map(|_| HandlerArmView { view: self, id })
    }

    pub fn pattern(&self, id: HirPatId) -> Option<PatternView<'_, 'hir>> {
        self.program
            .pats
            .get(id)
            .map(|_| PatternView { view: self, id })
    }

    pub fn scope(&self, id: ScopeId) -> Option<ScopeView<'_, 'hir>> {
        self.program
            .scopes
            .get(id)
            .map(|_| ScopeView { view: self, id })
    }

    pub fn enclosing_scope(&self, node: HirNodeRef) -> Option<ScopeId> {
        match node {
            HirNodeRef::Item(item) => {
                self.program
                    .items
                    .get(item)
                    .and_then(item_scope)
                    .or_else(|| {
                        self.index.item_module.get(&item).and_then(|module| {
                            self.program
                                .modules_arena
                                .get(*module)
                                .map(|module| module.scope)
                        })
                    })
            }
            HirNodeRef::Expr(expr) => self.expr_scope(expr).or_else(|| {
                self.index
                    .expr_owner
                    .get(&expr)
                    .and_then(|owner| self.scope_for_owner(*owner))
            }),
            HirNodeRef::HandlerArm(arm) => self.program.handler_arms.get(arm).map(|arm| arm.scope),
            HirNodeRef::Stmt(stmt) => self
                .index
                .stmt_block
                .get(&stmt)
                .and_then(|block| self.program.blocks.get(*block))
                .map(|block| block.scope),
            HirNodeRef::Pat(pat) => self
                .index
                .pat_owner
                .get(&pat)
                .and_then(|owner| self.scope_for_owner(*owner)),
            HirNodeRef::Block(block) => self.program.blocks.get(block).map(|block| block.scope),
            HirNodeRef::Type(_) | HirNodeRef::Symbol(_) => None,
        }
    }

    pub fn source_span(&self, node: HirNodeRef) -> Option<Span> {
        match node {
            HirNodeRef::Item(id) => self.program.items.get(id).map(HirItem::span),
            HirNodeRef::Expr(id) => self
                .program
                .exprs
                .get(id)
                .map(|expr| expr.span(&self.program.blocks)),
            HirNodeRef::HandlerArm(id) => self.program.handler_arms.get(id).map(|arm| arm.span),
            HirNodeRef::Stmt(id) => self
                .program
                .stmts
                .get(id)
                .and_then(|stmt| stmt_span(self.program, stmt)),
            HirNodeRef::Pat(id) => self.program.pats.get(id).map(HirPat::span),
            HirNodeRef::Type(id) => self.program.types.get(id).map(|ty| ty.span()),
            HirNodeRef::Block(id) => self.program.blocks.get(id).map(|block| block.span),
            HirNodeRef::Symbol(id) => self
                .program
                .source_map
                .symbol_sources
                .get(&id)
                .map(|origin| origin.span()),
        }
    }

    fn scope_for_owner(&self, owner: HirOwner) -> Option<ScopeId> {
        if let Some(scope) = self
            .index
            .scope_by_owner
            .get(&owner)
            .and_then(|scopes| scopes.first().copied())
        {
            return Some(scope);
        }
        match owner {
            HirOwner::Module(module) => self.program.modules_arena.get(module).map(|m| m.scope),
            HirOwner::Item(item) => self.enclosing_scope(HirNodeRef::Item(item)),
            HirOwner::Body(body) => body_scope(self.program, body),
            HirOwner::Block(block) => self.program.blocks.get(block).map(|block| block.scope),
            HirOwner::Stmt(stmt) => self.enclosing_scope(HirNodeRef::Stmt(stmt)),
            HirOwner::Expr(expr) => self.enclosing_scope(HirNodeRef::Expr(expr)),
            HirOwner::HandlerArm(arm) => self.enclosing_scope(HirNodeRef::HandlerArm(arm)),
            HirOwner::Pattern(pat) => self.enclosing_scope(HirNodeRef::Pat(pat)),
        }
    }

    fn expr_scope(&self, expr: HirExprId) -> Option<ScopeId> {
        match self.program.exprs.get(expr)? {
            HirExpr::Lambda { scope, .. } => Some(*scope),
            HirExpr::Handler { .. } => self
                .index
                .scope_by_owner
                .get(&HirOwner::Expr(expr))
                .and_then(|scopes| scopes.first().copied()),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
pub struct ModuleView<'view, 'hir> {
    view: &'view HirTreeView<'hir>,
    id: HirModuleId,
}

impl<'view, 'hir> ModuleView<'view, 'hir> {
    pub fn id(self) -> HirModuleId {
        self.id
    }

    pub fn data(self) -> &'hir HirModule {
        &self.view.program.modules_arena[self.id]
    }

    pub fn items(self) -> impl Iterator<Item = ItemView<'view, 'hir>> + 'view {
        let view = self.view;
        view.index
            .module_items
            .get(&self.id)
            .into_iter()
            .flat_map(|items| items.iter().copied())
            .filter_map(move |item| view.item(item))
    }

    pub fn scope(self) -> ScopeView<'view, 'hir> {
        self.view
            .scope(self.data().scope)
            .expect("module scope should exist")
    }
}

#[derive(Clone, Copy)]
pub struct ItemView<'view, 'hir> {
    view: &'view HirTreeView<'hir>,
    id: HirItemId,
}

impl<'view, 'hir> ItemView<'view, 'hir> {
    pub fn id(self) -> HirItemId {
        self.id
    }

    pub fn data(self) -> &'hir HirItem {
        &self.view.program.items[self.id]
    }

    pub fn module(self) -> Option<ModuleView<'view, 'hir>> {
        self.view
            .index
            .item_module
            .get(&self.id)
            .and_then(|module| self.view.module(*module))
    }

    pub fn body(self) -> Option<BodyView<'view, 'hir>> {
        self.view
            .index
            .item_body
            .get(&self.id)
            .copied()
            .and_then(|body| self.view.body(body))
    }

    pub fn bodies(self) -> impl Iterator<Item = BodyView<'view, 'hir>> + 'view {
        let view = self.view;
        view.index
            .item_bodies
            .get(&self.id)
            .into_iter()
            .flat_map(|bodies| bodies.iter().copied())
            .filter_map(move |body| view.body(body))
    }

    pub fn scope(self) -> Option<ScopeView<'view, 'hir>> {
        self.view
            .enclosing_scope(HirNodeRef::Item(self.id))
            .and_then(|scope| self.view.scope(scope))
    }
}

#[derive(Clone, Copy)]
pub struct BodyView<'view, 'hir> {
    view: &'view HirTreeView<'hir>,
    id: HirBodyRef,
}

impl<'view, 'hir> BodyView<'view, 'hir> {
    pub fn id(self) -> HirBodyRef {
        self.id
    }

    pub fn item(self) -> ItemView<'view, 'hir> {
        self.view
            .item(self.id.item)
            .expect("body item should exist in view")
    }

    pub fn owner(self) -> Option<HirOwner> {
        self.view.index.body_owner.get(&self.id).copied()
    }

    pub fn scope(self) -> Option<ScopeView<'view, 'hir>> {
        self.view
            .scope_for_owner(HirOwner::Body(self.id))
            .and_then(|scope| self.view.scope(scope))
    }

    pub fn blocks(self) -> impl Iterator<Item = BlockView<'view, 'hir>> + 'view {
        let view = self.view;
        view.index
            .body_blocks
            .get(&self.id)
            .into_iter()
            .flat_map(|blocks| blocks.iter().copied())
            .filter_map(move |block| view.block(block))
    }

    pub fn root_exprs(self) -> impl Iterator<Item = ExprView<'view, 'hir>> + 'view {
        let view = self.view;
        view.index
            .body_exprs
            .get(&self.id)
            .into_iter()
            .flat_map(|exprs| exprs.iter().copied())
            .filter_map(move |expr| view.expr(expr))
    }
}

#[derive(Clone, Copy)]
pub struct BlockView<'view, 'hir> {
    view: &'view HirTreeView<'hir>,
    id: HirBlockId,
}

impl<'view, 'hir> BlockView<'view, 'hir> {
    pub fn id(self) -> HirBlockId {
        self.id
    }

    pub fn data(self) -> &'hir HirBlock {
        &self.view.program.blocks[self.id]
    }

    pub fn owner(self) -> Option<HirOwner> {
        self.view.index.block_owner.get(&self.id).copied()
    }

    pub fn statements(self) -> impl Iterator<Item = StmtView<'view, 'hir>> + 'view {
        let view = self.view;
        self.data()
            .stmts
            .iter()
            .copied()
            .filter_map(move |stmt| view.stmt(stmt))
    }

    pub fn final_expr(self) -> Option<ExprView<'view, 'hir>> {
        self.data().final_expr.and_then(|expr| self.view.expr(expr))
    }

    pub fn scope(self) -> ScopeView<'view, 'hir> {
        self.view
            .scope(self.data().scope)
            .expect("block scope should exist")
    }
}

#[derive(Clone, Copy)]
pub struct StmtView<'view, 'hir> {
    view: &'view HirTreeView<'hir>,
    id: HirStmtId,
}

impl<'view, 'hir> StmtView<'view, 'hir> {
    pub fn id(self) -> HirStmtId {
        self.id
    }

    pub fn data(self) -> &'hir HirStmt {
        &self.view.program.stmts[self.id]
    }

    pub fn block(self) -> Option<BlockView<'view, 'hir>> {
        self.view
            .index
            .stmt_block
            .get(&self.id)
            .and_then(|block| self.view.block(*block))
    }

    pub fn for_each_child(self, f: impl FnMut(HirTreeChild)) {
        for_each_stmt_child(self.view.program, self.id, f);
    }

    pub fn children(self) -> impl Iterator<Item = HirTreeChild> + 'hir {
        stmt_children(self.view.program, self.id)
    }
}

#[derive(Clone, Copy)]
pub struct ExprView<'view, 'hir> {
    view: &'view HirTreeView<'hir>,
    id: HirExprId,
}

impl<'view, 'hir> ExprView<'view, 'hir> {
    pub fn id(self) -> HirExprId {
        self.id
    }

    pub fn data(self) -> &'hir HirExpr {
        &self.view.program.exprs[self.id]
    }

    pub fn owner(self) -> Option<HirOwner> {
        self.view.index.expr_owner.get(&self.id).copied()
    }

    pub fn for_each_child(self, f: impl FnMut(HirTreeChild)) {
        for_each_expr_child(self.view.program, self.id, f);
    }

    pub fn children(self) -> impl Iterator<Item = HirTreeChild> + 'hir {
        expr_children(self.view.program, self.id)
    }

    pub fn source_span(self) -> Span {
        self.data().span(&self.view.program.blocks)
    }
}

#[derive(Clone, Copy)]
pub struct HandlerArmView<'view, 'hir> {
    view: &'view HirTreeView<'hir>,
    id: HirHandlerArmId,
}

impl<'view, 'hir> HandlerArmView<'view, 'hir> {
    pub fn id(self) -> HirHandlerArmId {
        self.id
    }

    pub fn data(self) -> &'hir HirHandlerArm {
        &self.view.program.handler_arms[self.id]
    }

    pub fn body(self) -> BlockView<'view, 'hir> {
        self.view
            .block(self.data().body)
            .expect("handler arm body block should exist")
    }
}

#[derive(Clone, Copy)]
pub struct PatternView<'view, 'hir> {
    view: &'view HirTreeView<'hir>,
    id: HirPatId,
}

impl<'view, 'hir> PatternView<'view, 'hir> {
    pub fn id(self) -> HirPatId {
        self.id
    }

    pub fn data(self) -> &'hir HirPat {
        &self.view.program.pats[self.id]
    }

    pub fn owner(self) -> Option<HirOwner> {
        self.view.index.pat_owner.get(&self.id).copied()
    }

    pub fn for_each_child(self, f: impl FnMut(HirTreeChild)) {
        for_each_pat_child(self.data(), f);
    }

    pub fn children(self) -> impl Iterator<Item = HirTreeChild> + 'hir {
        pat_children(self.data())
    }
}

#[derive(Clone, Copy)]
pub struct ScopeView<'view, 'hir> {
    view: &'view HirTreeView<'hir>,
    id: ScopeId,
}

impl<'view, 'hir> ScopeView<'view, 'hir> {
    pub fn id(self) -> ScopeId {
        self.id
    }

    pub fn data(self) -> &'hir Scope {
        self.view
            .program
            .scopes
            .get(self.id)
            .expect("scope view should reference an existing scope")
    }

    pub fn children(self) -> impl Iterator<Item = ScopeView<'view, 'hir>> + 'view {
        let view = self.view;
        view.index
            .scope_children
            .get(&self.id)
            .into_iter()
            .flat_map(|scopes| scopes.iter().copied())
            .filter_map(move |scope| view.scope(scope))
    }
}
