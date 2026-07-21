use crate::{
    HirBlockId, HirExpr, HirExprId, HirFlowBody, HirHandlerArmId, HirItem, HirItemId,
    HirLambdaBody, HirMatchArmBody, HirModuleId, HirPatId, HirProgram, HirStmtId, HirToolBody,
    Scope, ScopeId, ScopeOwner,
};

use super::helpers::{
    body_debug_id, for_each_expr_child, for_each_pat_child, for_each_stmt_child, item_scope,
};
use super::model::{
    HirBodyKind, HirBodyRef, HirOwner, HirTreeChild, HirTreeIndex, HirTreeIndexDiagnostic,
    HirTreeIndexError, HirTreeNodeKind,
};

impl HirTreeIndex {
    pub fn try_build(program: &HirProgram) -> Result<Self, HirTreeIndexError> {
        let mut builder = HirTreeIndexBuilder {
            program,
            index: HirTreeIndex::default(),
            diagnostics: Vec::new(),
        };
        builder.build();
        builder.finish()
    }

    pub fn build(program: &HirProgram) -> Self {
        Self::try_build(program).expect("HIR tree invariants should hold")
    }
}

struct HirTreeIndexBuilder<'hir> {
    program: &'hir HirProgram,
    index: HirTreeIndex,
    diagnostics: Vec<HirTreeIndexDiagnostic>,
}

impl HirTreeIndexBuilder<'_> {
    fn finish(mut self) -> Result<HirTreeIndex, HirTreeIndexError> {
        self.validate_orphans();
        if self.diagnostics.is_empty() {
            Ok(self.index)
        } else {
            Err(HirTreeIndexError {
                diagnostics: self.diagnostics,
            })
        }
    }

    fn build(&mut self) {
        self.index_scopes();
        for module in &self.program.modules {
            let Some(module_data) = self.program.modules_arena.get(*module) else {
                self.missing(HirTreeNodeKind::Module, module.0, None);
                continue;
            };
            self.insert_scope_owner(HirOwner::Module(*module), module_data.scope);
            self.index
                .module_items
                .insert(*module, module_data.items.clone());
            for item in &module_data.items {
                if self.insert_item_module(*item, *module) {
                    self.index_item(*item);
                }
            }
        }
    }

    fn index_scopes(&mut self) {
        for scope in self.program.scopes.iter() {
            if let Some(parent) = scope.parent {
                if self.program.scopes.get(parent).is_none() {
                    self.diagnostics.push(HirTreeIndexDiagnostic::MissingScope {
                        scope: parent,
                        owner: None,
                    });
                    continue;
                }
                self.index
                    .scope_children
                    .entry(parent)
                    .or_default()
                    .push(scope.id);
            }
            self.index_scope_owner(scope);
        }
    }

    fn index_item(&mut self, item_id: HirItemId) {
        let Some(item) = self.program.items.get(item_id) else {
            self.missing(HirTreeNodeKind::Item, item_id.0, None);
            return;
        };
        if let Some(scope) = item_scope(item) {
            self.insert_scope_owner(HirOwner::Item(item_id), scope);
        }
        match item {
            HirItem::Flow(flow) => {
                let body = HirBodyRef::new(item_id, HirBodyKind::Flow);
                self.insert_body(item_id, body, HirOwner::Item(item_id), Some(flow.scope));
                self.index_flow_body(flow.body, body, item_id);
            }
            HirItem::Tool(tool) => {
                if let HirToolBody::Source(body_data) = tool.body {
                    let body = HirBodyRef::new(item_id, HirBodyKind::Tool);
                    self.insert_body(item_id, body, HirOwner::Item(item_id), Some(tool.scope));
                    self.index_flow_body(body_data, body, item_id);
                }
            }
            HirItem::Agent(agent) => {
                if let crate::HirAgentBody::Source { block } = agent.body {
                    let body = HirBodyRef::new(item_id, HirBodyKind::Agent);
                    self.insert_body(item_id, body, HirOwner::Item(item_id), Some(agent.scope));
                    self.index_block(block, HirOwner::Body(body), item_id);
                    self.index.body_blocks.entry(body).or_default().push(block);
                }
            }
            HirItem::TopLevelLet(value) => {
                let body = HirBodyRef::new(item_id, HirBodyKind::TopLevelLet);
                self.insert_body(item_id, body, HirOwner::Item(item_id), None);
                self.index_expr(value.value, HirOwner::Body(body), item_id);
                self.index
                    .body_exprs
                    .entry(body)
                    .or_default()
                    .push(value.value);
            }
            HirItem::Impl(item) => {
                for (index, impl_item) in item.items.iter().enumerate() {
                    if let crate::HirImplItem::Flow(flow) = impl_item {
                        let body = HirBodyRef::new(item_id, HirBodyKind::ImplFlow(index as u32));
                        self.insert_body(item_id, body, HirOwner::Item(item_id), Some(flow.scope));
                        self.index_flow_body(flow.body, body, item_id);
                    }
                }
            }
            HirItem::TypeAlias(_)
            | HirItem::Type(_)
            | HirItem::Enum(_)
            | HirItem::Spec(_)
            | HirItem::Effect(_)
            | HirItem::Protocol(_)
            | HirItem::Error { .. } => {}
        }
        self.index_item_annotations(item_id, item_scope(item));
    }

    fn index_item_annotations(&mut self, item_id: HirItemId, scope: Option<ScopeId>) {
        let Some(annotations) = self.program.item_annotations.get(&item_id) else {
            return;
        };
        for (annotation_index, annotation) in annotations.iter().enumerate() {
            for (arg_index, arg) in annotation.args.iter().enumerate() {
                let value = match arg {
                    crate::HirAnnotationArg::Positional { value, .. }
                    | crate::HirAnnotationArg::Named { value, .. } => *value,
                };
                let body = HirBodyRef::new(
                    item_id,
                    HirBodyKind::AnnotationArg {
                        annotation: annotation_index as u32,
                        arg: arg_index as u32,
                    },
                );
                self.insert_body(item_id, body, HirOwner::Item(item_id), scope);
                self.index_expr(value, HirOwner::Body(body), item_id);
                self.index.body_exprs.entry(body).or_default().push(value);
            }
        }
    }

    fn index_flow_body(&mut self, body_data: HirFlowBody, body: HirBodyRef, item_id: HirItemId) {
        match body_data {
            HirFlowBody::Block(block) => {
                self.index_block(block, HirOwner::Body(body), item_id);
                self.index.body_blocks.entry(body).or_default().push(block);
            }
            HirFlowBody::Expr { lowered_block, .. } => {
                self.index_block(lowered_block, HirOwner::Body(body), item_id);
                self.index
                    .body_blocks
                    .entry(body)
                    .or_default()
                    .push(lowered_block);
            }
        }
    }

    fn index_block(&mut self, block_id: HirBlockId, owner: HirOwner, item_id: HirItemId) {
        if !self.insert_block_owner(block_id, owner) {
            return;
        }
        let Some(block) = self.program.blocks.get(block_id) else {
            self.missing(HirTreeNodeKind::Block, block_id.0, Some(owner));
            return;
        };
        self.insert_scope_owner(HirOwner::Block(block_id), block.scope);
        for stmt in &block.stmts {
            if self.insert_stmt_block(*stmt, block_id) {
                self.index_stmt(*stmt, item_id);
            }
        }
        if let Some(expr) = block.final_expr {
            self.index_expr(expr, HirOwner::Block(block_id), item_id);
        }
    }

    fn index_stmt(&mut self, stmt_id: HirStmtId, item_id: HirItemId) {
        if self.program.stmts.get(stmt_id).is_none() {
            self.missing(HirTreeNodeKind::Statement, stmt_id.0, None);
            return;
        }
        let program = self.program;
        for_each_stmt_child(program, stmt_id, |child| match child {
            HirTreeChild::Expr(expr) => self.index_expr(expr, HirOwner::Stmt(stmt_id), item_id),
            HirTreeChild::Block(block) => self.index_block(block, HirOwner::Stmt(stmt_id), item_id),
            HirTreeChild::Pattern(pat) => self.index_pat(pat, HirOwner::Stmt(stmt_id)),
            HirTreeChild::Stmt(_) | HirTreeChild::HandlerArm(_) => {}
        });
    }

    fn index_expr(&mut self, expr_id: HirExprId, owner: HirOwner, item_id: HirItemId) {
        let mut stack = vec![(expr_id, owner)];
        let mut children = Vec::new();
        while let Some((expr_id, owner)) = stack.pop() {
            if !self.insert_expr_owner(expr_id, owner) {
                continue;
            }
            self.index_expr_body(expr_id, item_id, owner, &mut stack);

            children.clear();
            let program = self.program;
            for_each_expr_child(program, expr_id, |child| children.push(child));
            for child in children.iter().rev().copied() {
                match child {
                    HirTreeChild::Expr(expr) => stack.push((expr, HirOwner::Expr(expr_id))),
                    HirTreeChild::Block(block) => {
                        self.index_block(block, HirOwner::Expr(expr_id), item_id);
                    }
                    HirTreeChild::HandlerArm(arm) => {
                        self.index_handler_arm(arm, HirOwner::Expr(expr_id), item_id);
                    }
                    HirTreeChild::Pattern(pat) => self.index_pat(pat, HirOwner::Expr(expr_id)),
                    HirTreeChild::Stmt(stmt) => self.index_stmt(stmt, item_id),
                }
            }
        }
    }

    fn index_pat(&mut self, pat_id: HirPatId, owner: HirOwner) {
        if !self.insert_pat_owner(pat_id, owner) {
            return;
        }
        let Some(pat) = self.program.pats.get(pat_id) else {
            self.missing(HirTreeNodeKind::Pattern, pat_id.0, Some(owner));
            return;
        };
        for_each_pat_child(pat, |child| {
            if let HirTreeChild::Pattern(child) = child {
                self.index_pat(child, HirOwner::Pattern(pat_id));
            }
        });
    }

    fn index_handler_arm(&mut self, arm: HirHandlerArmId, owner: HirOwner, item_id: HirItemId) {
        if self.insert_handler_arm_owner(arm, owner)
            && let Some(arm_data) = self.program.handler_arms.get(arm)
        {
            self.insert_scope_owner(HirOwner::HandlerArm(arm), arm_data.scope);
            for pat in &arm_data.patterns {
                self.index_pat(*pat, HirOwner::HandlerArm(arm));
            }
            let body = HirBodyRef::new(item_id, HirBodyKind::HandlerArm(arm));
            self.insert_body(
                item_id,
                body,
                HirOwner::HandlerArm(arm),
                Some(arm_data.scope),
            );
            self.index_block(arm_data.body, HirOwner::Body(body), item_id);
            self.index
                .body_blocks
                .entry(body)
                .or_default()
                .push(arm_data.body);
        } else {
            self.missing(HirTreeNodeKind::HandlerArm, arm.0, Some(owner));
        }
    }

    fn index_expr_body(
        &mut self,
        expr_id: HirExprId,
        item_id: HirItemId,
        owner: HirOwner,
        expr_stack: &mut Vec<(HirExprId, HirOwner)>,
    ) {
        let Some(expr) = self.program.exprs.get(expr_id) else {
            self.missing(HirTreeNodeKind::Expr, expr_id.0, None);
            return;
        };
        match expr {
            HirExpr::Lambda { body, scope, .. } => {
                let body_ref = HirBodyRef::new(item_id, HirBodyKind::Lambda(expr_id));
                self.insert_body(item_id, body_ref, HirOwner::Expr(expr_id), Some(*scope));
                match body {
                    HirLambdaBody::Expr(expr) => {
                        self.index
                            .body_exprs
                            .entry(body_ref)
                            .or_default()
                            .push(*expr);
                        expr_stack.push((*expr, HirOwner::Body(body_ref)));
                    }
                    HirLambdaBody::Block(block) => {
                        self.index_block(*block, HirOwner::Body(body_ref), item_id);
                        self.index
                            .body_blocks
                            .entry(body_ref)
                            .or_default()
                            .push(*block);
                    }
                }
            }
            HirExpr::Match { arms, .. } => {
                for (arm_index, arm) in arms.iter().enumerate() {
                    let body_ref = HirBodyRef::new(
                        item_id,
                        HirBodyKind::MatchArm {
                            expr: expr_id,
                            arm: arm_index as u32,
                        },
                    );
                    self.insert_body(item_id, body_ref, HirOwner::Expr(expr_id), Some(arm.scope));
                    match arm.body {
                        HirMatchArmBody::Expr(expr) => {
                            self.index
                                .body_exprs
                                .entry(body_ref)
                                .or_default()
                                .push(expr);
                            expr_stack.push((expr, HirOwner::Body(body_ref)));
                        }
                        HirMatchArmBody::Block(block) => {
                            self.index_block(block, HirOwner::Body(body_ref), item_id);
                            self.index
                                .body_blocks
                                .entry(body_ref)
                                .or_default()
                                .push(block);
                        }
                    }
                }
            }
            HirExpr::StageCompose { stages, .. } | HirExpr::Pipeline { stages, .. } => {
                for (stage_index, stage) in stages.iter().enumerate() {
                    for (limit_index, limit) in stage.limits.iter().enumerate() {
                        let body_ref = HirBodyRef::new(
                            item_id,
                            HirBodyKind::StageLimit {
                                expr: expr_id,
                                stage: stage_index as u32,
                                limit: limit_index as u32,
                            },
                        );
                        self.insert_body(
                            item_id,
                            body_ref,
                            HirOwner::Expr(expr_id),
                            self.scope_for_index_owner(owner),
                        );
                        self.index
                            .body_exprs
                            .entry(body_ref)
                            .or_default()
                            .push(*limit);
                        expr_stack.push((*limit, HirOwner::Body(body_ref)));
                    }
                }
            }
            _ => {}
        }
    }

    fn insert_body(
        &mut self,
        item: HirItemId,
        body: HirBodyRef,
        owner: HirOwner,
        scope: Option<ScopeId>,
    ) -> bool {
        self.index.item_body.entry(item).or_insert(body);
        self.index.item_bodies.entry(item).or_default().push(body);
        if let Some(previous) = self.index.body_owner.insert(body, owner) {
            self.diagnostics
                .push(HirTreeIndexDiagnostic::DuplicateOwner {
                    kind: HirTreeNodeKind::Body,
                    id: body_debug_id(body),
                    previous,
                    next: owner,
                });
            return false;
        }
        if let Some(scope) = scope {
            self.insert_scope_owner(HirOwner::Body(body), scope);
        }
        true
    }

    fn insert_item_module(&mut self, item: HirItemId, module: HirModuleId) -> bool {
        if self.program.items.get(item).is_none() {
            self.missing(
                HirTreeNodeKind::Item,
                item.0,
                Some(HirOwner::Module(module)),
            );
            return false;
        }
        let previous = self.index.item_module.insert(item, module);
        if let Some(previous) = previous
            && previous != module
        {
            self.diagnostics
                .push(HirTreeIndexDiagnostic::DuplicateOwner {
                    kind: HirTreeNodeKind::Item,
                    id: item.0,
                    previous: HirOwner::Module(previous),
                    next: HirOwner::Module(module),
                });
            return false;
        }
        true
    }

    fn insert_block_owner(&mut self, block: HirBlockId, owner: HirOwner) -> bool {
        let previous = self.index.block_owner.insert(block, owner);
        if let Some(previous) = previous
            && previous != owner
        {
            self.diagnostics
                .push(HirTreeIndexDiagnostic::DuplicateOwner {
                    kind: HirTreeNodeKind::Block,
                    id: block.0,
                    previous,
                    next: owner,
                });
            return false;
        }
        true
    }

    fn insert_stmt_block(&mut self, stmt: HirStmtId, block: HirBlockId) -> bool {
        let previous = self.index.stmt_block.insert(stmt, block);
        if let Some(previous) = previous
            && previous != block
        {
            self.diagnostics
                .push(HirTreeIndexDiagnostic::DuplicateOwner {
                    kind: HirTreeNodeKind::Statement,
                    id: stmt.0,
                    previous: HirOwner::Block(previous),
                    next: HirOwner::Block(block),
                });
            return false;
        }
        true
    }

    fn insert_expr_owner(&mut self, expr: HirExprId, owner: HirOwner) -> bool {
        if self.program.exprs.get(expr).is_none() {
            self.missing(HirTreeNodeKind::Expr, expr.0, Some(owner));
            return false;
        }
        let previous = self.index.expr_owner.insert(expr, owner);
        if let Some(previous) = previous
            && previous != owner
        {
            self.diagnostics
                .push(HirTreeIndexDiagnostic::DuplicateOwner {
                    kind: HirTreeNodeKind::Expr,
                    id: expr.0,
                    previous,
                    next: owner,
                });
            return false;
        }
        true
    }

    fn insert_handler_arm_owner(&mut self, arm: HirHandlerArmId, owner: HirOwner) -> bool {
        if self.program.handler_arms.get(arm).is_none() {
            self.missing(HirTreeNodeKind::HandlerArm, arm.0, Some(owner));
            return false;
        }
        let previous = self.index.handler_arm_owner.insert(arm, owner);
        if let Some(previous) = previous
            && previous != owner
        {
            self.diagnostics
                .push(HirTreeIndexDiagnostic::DuplicateOwner {
                    kind: HirTreeNodeKind::HandlerArm,
                    id: arm.0,
                    previous,
                    next: owner,
                });
            return false;
        }
        true
    }

    fn insert_pat_owner(&mut self, pat: HirPatId, owner: HirOwner) -> bool {
        let previous = self.index.pat_owner.insert(pat, owner);
        if let Some(previous) = previous
            && previous != owner
        {
            self.diagnostics
                .push(HirTreeIndexDiagnostic::DuplicateOwner {
                    kind: HirTreeNodeKind::Pattern,
                    id: pat.0,
                    previous,
                    next: owner,
                });
            return false;
        }
        true
    }

    fn index_scope_owner(&mut self, scope: &Scope) {
        let owner = match scope.owner {
            ScopeOwner::Module(module) => HirOwner::Module(module),
            ScopeOwner::Item(item) => HirOwner::Item(item),
            ScopeOwner::Block(block) => HirOwner::Block(block),
            ScopeOwner::Lambda(expr) => HirOwner::Expr(expr),
            ScopeOwner::Handler(expr) | ScopeOwner::MatchArm(expr) => HirOwner::Expr(expr),
        };
        self.insert_scope_owner(owner, scope.id);
    }

    fn insert_scope_owner(&mut self, owner: HirOwner, scope: ScopeId) {
        if self.program.scopes.get(scope).is_none() {
            self.diagnostics.push(HirTreeIndexDiagnostic::MissingScope {
                scope,
                owner: Some(owner),
            });
            return;
        }
        let scopes = self.index.scope_by_owner.entry(owner).or_default();
        if !scopes.contains(&scope) {
            scopes.push(scope);
        }
    }

    fn scope_for_index_owner(&self, owner: HirOwner) -> Option<ScopeId> {
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
            HirOwner::Item(item) => self.program.items.get(item).and_then(item_scope),
            HirOwner::Body(body) => self
                .index
                .scope_by_owner
                .get(&HirOwner::Body(body))
                .and_then(|scopes| scopes.first().copied()),
            HirOwner::Block(block) => self.program.blocks.get(block).map(|block| block.scope),
            HirOwner::Stmt(stmt) => self
                .index
                .stmt_block
                .get(&stmt)
                .and_then(|block| self.scope_for_index_owner(HirOwner::Block(*block))),
            HirOwner::Expr(expr) => self
                .index
                .expr_owner
                .get(&expr)
                .and_then(|owner| self.scope_for_index_owner(*owner)),
            HirOwner::HandlerArm(arm) => self.program.handler_arms.get(arm).map(|arm| arm.scope),
            HirOwner::Pattern(pat) => self
                .index
                .pat_owner
                .get(&pat)
                .and_then(|owner| self.scope_for_index_owner(*owner)),
        }
    }

    fn missing(&mut self, kind: HirTreeNodeKind, id: u32, owner: Option<HirOwner>) {
        self.diagnostics
            .push(HirTreeIndexDiagnostic::MissingNode { kind, id, owner });
    }

    fn validate_orphans(&mut self) {
        for (module, _) in self.program.modules_arena.iter() {
            if !self.program.modules.contains(&module) {
                self.diagnostics.push(HirTreeIndexDiagnostic::OrphanNode {
                    kind: HirTreeNodeKind::Module,
                    id: module.0,
                });
            }
        }
        for (item, _) in self.program.items.iter() {
            if !self.index.item_module.contains_key(&item) {
                self.diagnostics.push(HirTreeIndexDiagnostic::OrphanNode {
                    kind: HirTreeNodeKind::Item,
                    id: item.0,
                });
            }
        }
        for (block, _) in self.program.blocks.iter() {
            if !self.index.block_owner.contains_key(&block) {
                self.diagnostics.push(HirTreeIndexDiagnostic::OrphanNode {
                    kind: HirTreeNodeKind::Block,
                    id: block.0,
                });
            }
        }
        for (stmt, _) in self.program.stmts.iter() {
            if !self.index.stmt_block.contains_key(&stmt) {
                self.diagnostics.push(HirTreeIndexDiagnostic::OrphanNode {
                    kind: HirTreeNodeKind::Statement,
                    id: stmt.0,
                });
            }
        }
        for (expr, _) in self.program.exprs.iter() {
            if !self.index.expr_owner.contains_key(&expr) {
                self.diagnostics.push(HirTreeIndexDiagnostic::OrphanNode {
                    kind: HirTreeNodeKind::Expr,
                    id: expr.0,
                });
            }
        }
        for (arm, _) in self.program.handler_arms.iter() {
            if !self.index.handler_arm_owner.contains_key(&arm) {
                self.diagnostics.push(HirTreeIndexDiagnostic::OrphanNode {
                    kind: HirTreeNodeKind::HandlerArm,
                    id: arm.0,
                });
            }
        }
        for (pat, _) in self.program.pats.iter() {
            if !self.index.pat_owner.contains_key(&pat) {
                self.diagnostics.push(HirTreeIndexDiagnostic::OrphanNode {
                    kind: HirTreeNodeKind::Pattern,
                    id: pat.0,
                });
            }
        }
    }
}
