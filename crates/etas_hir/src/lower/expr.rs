use super::program::LowerCtx;
use crate::*;
use etas_syntax::ast::{self, *};

impl LowerCtx {
    pub(super) fn lower_expr(
        &mut self,
        expr: &Expr,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirExprId {
        let span = expr.span();
        match expr {
            Expr::Handle(handle) => return self.lower_handle_expr(handle, scope, item_id),
            Expr::Match(match_expr) => return self.lower_match_expr(match_expr, scope, item_id),
            Expr::Lambda(lambda) => return self.lower_lambda_expr(lambda, scope, item_id),
            Expr::MethodCall(_) => return self.lower_method_call_chain(expr, scope, item_id),
            Expr::Binary(binary) if logical_chain_op(binary.op) => {
                return self.lower_logical_binary_chain(expr, binary.op, scope, item_id);
            }
            _ => {}
        }

        let hir_expr = match expr {
            Expr::Literal(lit) => HirExpr::Literal(match lit {
                Literal::Bool { value, span } => HirLiteral::Bool {
                    value: *value,
                    span: *span,
                },
                Literal::Int { text, span } => HirLiteral::Int {
                    text: text.clone(),
                    span: *span,
                },
                Literal::Float { text, span } => HirLiteral::Float {
                    text: text.clone(),
                    span: *span,
                },
                Literal::String { value, span } => HirLiteral::String {
                    value: value.clone(),
                    span: *span,
                },
                Literal::Char { value, span } => HirLiteral::Char {
                    value: *value,
                    span: *span,
                },
            }),
            Expr::Path(path) => HirExpr::Path(self.resolve_path(path, scope, true)),
            Expr::Record(record) => HirExpr::Record(HirRecordExpr {
                path: record
                    .path
                    .as_ref()
                    .map(|path| self.resolve_path(path, scope, true)),
                generic_args: record
                    .generic_args
                    .iter()
                    .map(|arg| self.lower_generic_arg(arg, scope, item_id))
                    .collect(),
                fields: record
                    .fields
                    .iter()
                    .map(|field| self.lower_field_init(field, scope, item_id))
                    .collect(),
                span: record.span,
            }),
            Expr::EmptyRecordOrMap { span } => HirExpr::EmptyRecordOrMap { span: *span },
            Expr::Tuple { elems, span } => HirExpr::Tuple {
                elems: elems
                    .iter()
                    .map(|elem| self.lower_expr(elem, scope, item_id))
                    .collect(),
                span: *span,
            },
            Expr::Array { elems, span } => HirExpr::Array {
                elems: elems
                    .iter()
                    .map(|elem| self.lower_expr(elem, scope, item_id))
                    .collect(),
                span: *span,
            },
            Expr::List { elems, span } => HirExpr::List {
                elems: elems
                    .iter()
                    .map(|elem| self.lower_expr(elem, scope, item_id))
                    .collect(),
                span: *span,
            },
            Expr::ListCons { head, tail, span } => HirExpr::ListCons {
                head: self.lower_expr(head, scope, item_id),
                tail: self.lower_expr(tail, scope, item_id),
                span: *span,
            },
            Expr::EmptySequence { span } => HirExpr::EmptySequence { span: *span },
            Expr::Map(map) => HirExpr::Map {
                entries: map
                    .entries
                    .iter()
                    .map(|entry| HirMapEntry {
                        key: self.lower_expr(&entry.key, scope, item_id),
                        value: self.lower_expr(&entry.value, scope, item_id),
                        span: entry.span,
                    })
                    .collect(),
                span: map.span,
            },
            Expr::Set { elems, span } => HirExpr::Set {
                elems: elems
                    .iter()
                    .map(|elem| self.lower_expr(elem, scope, item_id))
                    .collect(),
                span: *span,
            },
            Expr::Range(range) => HirExpr::Range {
                start: self.lower_expr(&range.start, scope, item_id),
                end: self.lower_expr(&range.end, scope, item_id),
                bounds: match range.bounds {
                    RangeBounds::ClosedOpen => HirRangeBounds::ClosedOpen,
                    RangeBounds::OpenClosed => HirRangeBounds::OpenClosed,
                },
                span: range.span,
            },
            Expr::Call(call) => HirExpr::Call {
                callee: self.lower_expr(&call.callee, scope, item_id),
                generic_args: call
                    .generic_args
                    .iter()
                    .map(|arg| self.lower_generic_arg(arg, scope, item_id))
                    .collect(),
                args: call
                    .args
                    .iter()
                    .map(|arg| self.lower_arg(arg, scope, item_id))
                    .collect(),
                span: call.span,
            },
            Expr::MethodCall(call) => HirExpr::MethodCall {
                receiver: self.lower_expr(&call.receiver, scope, item_id),
                method: call.method.text.clone(),
                generic_args: call
                    .generic_args
                    .iter()
                    .map(|arg| self.lower_generic_arg(arg, scope, item_id))
                    .collect(),
                args: call
                    .args
                    .iter()
                    .map(|arg| self.lower_arg(arg, scope, item_id))
                    .collect(),
                span: call.span,
            },
            Expr::SpecMethodCall(call) => HirExpr::SpecMethodCall {
                receiver: self.lower_expr(&call.receiver, scope, item_id),
                spec_path: self.resolve_path(&call.spec_path, scope, true),
                spec_args: call
                    .spec_args
                    .iter()
                    .map(|arg| self.lower_type(arg, scope, item_id))
                    .collect(),
                method: call.method.text.clone(),
                args: call
                    .args
                    .iter()
                    .map(|arg| self.lower_arg(arg, scope, item_id))
                    .collect(),
                span: call.span,
            },
            Expr::Perform(perform) => {
                let effect = self.lower_effect_ref(&perform.effect, scope, item_id);
                HirExpr::Perform {
                    action: self.resolve_action(effect, &perform.action),
                    generic_args: perform
                        .generic_args
                        .iter()
                        .map(|arg| self.lower_generic_arg(arg, scope, item_id))
                        .collect(),
                    args: perform
                        .args
                        .iter()
                        .map(|arg| self.lower_arg(arg, scope, item_id))
                        .collect(),
                    span: perform.span,
                }
            }
            Expr::Handle(_) => {
                unreachable!("handle expressions are lowered with a preallocated id")
            }
            Expr::StageCompose(compose) => HirExpr::StageCompose {
                stages: compose
                    .stages
                    .iter()
                    .map(|stage| self.lower_stage(stage, scope, item_id))
                    .collect(),
                span: compose.span,
            },
            Expr::Pipeline(pipeline) => HirExpr::Pipeline {
                input: self.lower_expr(&pipeline.input, scope, item_id),
                stages: pipeline
                    .stages
                    .iter()
                    .map(|stage| self.lower_stage(stage, scope, item_id))
                    .collect(),
                span: pipeline.span,
            },
            Expr::Field(field) => HirExpr::Field {
                base: self.lower_expr(&field.receiver, scope, item_id),
                field: field.field.text.clone(),
                span: field.span,
            },
            Expr::Index(index) => HirExpr::Index {
                base: self.lower_expr(&index.receiver, scope, item_id),
                index: self.lower_expr(&index.index, scope, item_id),
                span: index.span,
            },
            Expr::Slice(slice) => HirExpr::Slice {
                base: self.lower_expr(&slice.receiver, scope, item_id),
                start: self.lower_expr(&slice.start, scope, item_id),
                end: self.lower_expr(&slice.end, scope, item_id),
                bounds: match slice.bounds {
                    etas_syntax::ast::RangeBounds::ClosedOpen => HirRangeBounds::ClosedOpen,
                    etas_syntax::ast::RangeBounds::OpenClosed => HirRangeBounds::OpenClosed,
                },
                span: slice.span,
            },
            Expr::Try(try_expr) => HirExpr::Try {
                expr: self.lower_expr(&try_expr.expr, scope, item_id),
                span: try_expr.span,
            },
            Expr::Unary(unary) => HirExpr::Unary {
                op: unary.op,
                expr: self.lower_expr(&unary.expr, scope, item_id),
                span: unary.span,
            },
            Expr::Binary(binary) => HirExpr::Binary {
                op: binary.op,
                lhs: self.lower_expr(&binary.lhs, scope, item_id),
                rhs: self.lower_expr(&binary.rhs, scope, item_id),
                span: binary.span,
            },
            Expr::If(if_expr) => HirExpr::If {
                cond: self.lower_expr(&if_expr.condition, scope, item_id),
                then_block: self.lower_block(&if_expr.then_branch, scope, item_id),
                else_branch: Some(HirElseBranch::Block(self.lower_block(
                    &if_expr.else_branch,
                    scope,
                    item_id,
                ))),
                span: if_expr.span,
            },
            Expr::Match(_) => unreachable!("match expressions are lowered with a preallocated id"),
            Expr::Lambda(_) => {
                unreachable!("lambda expressions are lowered with a preallocated id")
            }
            Expr::Block(block) => HirExpr::Block(self.lower_block(&block.block, scope, item_id)),
            Expr::Handler(handler) => {
                let id = self.hir.exprs.alloc(HirExpr::Error { span: handler.span });
                self.source_map.map_expr(id, handler.span);
                let handlers = self.lower_handler_arms(&handler.block, id, scope, item_id);
                *self.hir.exprs.get_mut(id).unwrap() = HirExpr::Handler {
                    handlers,
                    span: handler.span,
                };
                return id;
            }
            Expr::Error(span) => HirExpr::Error { span: *span },
        };
        self.alloc_expr(hir_expr, span)
    }

    fn lower_method_call_chain(
        &mut self,
        expr: &Expr,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirExprId {
        let mut calls = Vec::new();
        let mut current = expr;
        while let Expr::MethodCall(call) = current {
            calls.push(call);
            current = call.receiver.as_ref();
        }

        let mut receiver = self.lower_expr(current, scope, item_id);
        for call in calls.into_iter().rev() {
            let hir_expr = HirExpr::MethodCall {
                receiver,
                method: call.method.text.clone(),
                generic_args: call
                    .generic_args
                    .iter()
                    .map(|arg| self.lower_generic_arg(arg, scope, item_id))
                    .collect(),
                args: call
                    .args
                    .iter()
                    .map(|arg| self.lower_arg(arg, scope, item_id))
                    .collect(),
                span: call.span,
            };
            receiver = self.alloc_expr(hir_expr, call.span);
        }
        receiver
    }

    fn lower_logical_binary_chain(
        &mut self,
        expr: &Expr,
        op: BinaryOp,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirExprId {
        let mut leaves = Vec::new();
        let mut current = expr;
        loop {
            match current {
                Expr::Binary(binary) if binary.op == op => {
                    leaves.push(binary.rhs.as_ref());
                    current = binary.lhs.as_ref();
                }
                _ => {
                    leaves.push(current);
                    break;
                }
            }
        }
        leaves.reverse();

        let mut nodes = leaves
            .into_iter()
            .map(|leaf| {
                let id = self.lower_expr(leaf, scope, item_id);
                (id, leaf.span())
            })
            .collect::<Vec<_>>();

        while nodes.len() > 1 {
            let mut next = Vec::with_capacity(nodes.len().div_ceil(2));
            let mut iter = nodes.into_iter();
            while let Some((lhs, lhs_span)) = iter.next() {
                if let Some((rhs, rhs_span)) = iter.next() {
                    let span = lhs_span.cover(rhs_span);
                    let id = self.alloc_expr(HirExpr::Binary { op, lhs, rhs, span }, span);
                    next.push((id, span));
                } else {
                    next.push((lhs, lhs_span));
                }
            }
            nodes = next;
        }

        nodes
            .pop()
            .map(|(id, _)| id)
            .expect("logical binary chain should contain at least one leaf")
    }

    pub(super) fn alloc_expr(&mut self, hir_expr: HirExpr, span: etas_core::Span) -> HirExprId {
        let id = self.hir.exprs.alloc(hir_expr);
        self.source_map.map_expr(id, span);
        id
    }

    fn lower_handle_expr(
        &mut self,
        handle: &ast::HandleExpr,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirExprId {
        let id = self.hir.exprs.alloc(HirExpr::Error { span: handle.span });
        self.source_map.map_expr(id, handle.span);
        let body = self.lower_expr(&handle.body, scope, item_id);
        let handler = self.lower_handler_arg(&handle.handler, scope, item_id);
        *self.hir.exprs.get_mut(id).unwrap() = HirExpr::Handle {
            body,
            handler,
            span: handle.span,
        };
        id
    }

    fn lower_handler_arms(
        &mut self,
        block: &ast::HandlerBlock,
        owner: HirExprId,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> Vec<HirHandlerArmId> {
        block
            .arms
            .iter()
            .map(|arm| {
                let handler_scope =
                    self.scopes
                        .alloc(Some(scope), ScopeOwner::Handler(owner), arm.span);
                let previous_binding = self.pattern_binding;
                self.pattern_binding = Some(super::program::PatternBindingContext {
                    owner: PatternBindingOwner::Handler { expr: owner },
                    ty: None,
                    initializer: None,
                });
                let patterns = arm
                    .patterns
                    .iter()
                    .map(|pat| self.lower_pattern(pat, handler_scope, item_id))
                    .collect();
                self.pattern_binding = previous_binding;
                let effect = self.lower_effect_ref(&arm.effect, scope, item_id);
                let handler = HirHandlerArm {
                    action: self.resolve_action(effect, &arm.action),
                    generic_args: arm
                        .generic_args
                        .iter()
                        .map(|arg| self.lower_generic_arg(arg, handler_scope, item_id))
                        .collect(),
                    patterns,
                    body: self.lower_handler_arm_body(&arm.body, handler_scope, item_id),
                    scope: handler_scope,
                    span: arm.span,
                };
                let id = self.hir.handler_arms.alloc(handler);
                self.source_map.map_handler_arm(id, arm.span);
                id
            })
            .collect()
    }

    pub(super) fn lower_handler_arg(
        &mut self,
        arg: &ast::HandlerArg,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirExprId {
        match arg {
            ast::HandlerArg::Expr(expr) => self.lower_expr(expr, scope, item_id),
            ast::HandlerArg::Block(block) => {
                let id = self.hir.exprs.alloc(HirExpr::Error { span: block.span });
                self.source_map.map_expr(id, block.span);
                let handlers = self.lower_handler_arms(block, id, scope, item_id);
                *self.hir.exprs.get_mut(id).unwrap() = HirExpr::Handler {
                    handlers,
                    span: block.span,
                };
                id
            }
        }
    }

    pub(super) fn lower_handler_arm_body(
        &mut self,
        body: &ast::HandlerArmBody,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirBlockId {
        match body {
            ast::HandlerArmBody::Block(block) => self.lower_block(block, scope, item_id),
            ast::HandlerArmBody::Stmt(stmt) => self.lower_stmt_body_block(stmt, scope, item_id),
        }
    }

    pub(super) fn lower_stmt_body_block(
        &mut self,
        stmt: &ast::Stmt,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirBlockId {
        let span = stmt.span();
        let block_id = self.hir.blocks.alloc(HirBlock {
            id: HirBlockId(0),
            stmts: Vec::new(),
            final_expr: None,
            scope: ScopeId(0),
            span,
        });
        let block_scope = self
            .scopes
            .alloc(Some(scope), ScopeOwner::Block(block_id), span);
        let lowered_stmt = self.lower_stmt(stmt, block_scope, item_id);
        self.source_map.map_block(block_id, span);
        *self.hir.blocks.get_mut(block_id).unwrap() = HirBlock {
            id: block_id,
            stmts: vec![lowered_stmt],
            final_expr: None,
            scope: block_scope,
            span,
        };
        block_id
    }

    pub(super) fn lower_expr_body_block(
        &mut self,
        expr: &ast::Expr,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirBlockId {
        let span = expr.span();
        let block_id = self.hir.blocks.alloc(HirBlock {
            id: HirBlockId(0),
            stmts: Vec::new(),
            final_expr: None,
            scope: ScopeId(0),
            span,
        });
        let block_scope = self
            .scopes
            .alloc(Some(scope), ScopeOwner::Block(block_id), span);
        let lowered_expr = self.lower_expr(expr, block_scope, item_id);
        self.source_map.map_block(block_id, span);
        *self.hir.blocks.get_mut(block_id).unwrap() = HirBlock {
            id: block_id,
            stmts: Vec::new(),
            final_expr: Some(lowered_expr),
            scope: block_scope,
            span,
        };
        block_id
    }

    fn lower_match_expr(
        &mut self,
        match_expr: &ast::MatchExpr,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirExprId {
        let id = self.hir.exprs.alloc(HirExpr::Error {
            span: match_expr.span,
        });
        self.source_map.map_expr(id, match_expr.span);
        let scrutinee = self.lower_expr(&match_expr.scrutinee, scope, item_id);
        let arms = match_expr
            .arms
            .iter()
            .map(|arm| self.lower_match_arm(arm, scope, item_id, id))
            .collect();
        *self.hir.exprs.get_mut(id).unwrap() = HirExpr::Match {
            scrutinee,
            arms,
            span: match_expr.span,
        };
        id
    }

    fn lower_lambda_expr(
        &mut self,
        lambda: &ast::LambdaExpr,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirExprId {
        let id = self.hir.exprs.alloc(HirExpr::Error { span: lambda.span });
        self.source_map.map_expr(id, lambda.span);
        let lambda_scope = self
            .scopes
            .alloc(Some(scope), ScopeOwner::Lambda(id), lambda.span);
        let params = match &lambda.params {
            LambdaParams::Ident(name) => vec![self.bind_symbol_with_def(
                lambda_scope,
                SymbolData {
                    name: name.text.clone(),
                    kind: SymbolKind::Param,
                    visibility: crate::Visibility::Private,
                    defining_module: self.current_module,
                    defining_item: Some(item_id),
                    def: SymbolDef::Param {
                        owner: item_id,
                        param_index: 0,
                        pattern: None,
                        ty: None,
                    },
                    declared_type: None,
                    definition_span: name.span,
                },
            )],
            LambdaParams::ParamList(params) => self.lower_params(params, lambda_scope, item_id),
        };
        let body = match &lambda.body {
            LambdaBody::Expr(expr) => {
                HirLambdaBody::Expr(self.lower_expr(expr, lambda_scope, item_id))
            }
            LambdaBody::Block(block) => {
                HirLambdaBody::Block(self.lower_block(block, lambda_scope, item_id))
            }
        };
        *self.hir.exprs.get_mut(id).unwrap() = HirExpr::Lambda {
            params,
            body,
            scope: lambda_scope,
            span: lambda.span,
        };
        id
    }

    pub(super) fn lower_stage(
        &mut self,
        stage: &ast::PipelineStage,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirStage {
        HirStage {
            expr: self.lower_expr(&stage.expr, scope, item_id),
            limits: stage
                .limits
                .iter()
                .map(|limit| self.lower_expr(limit, scope, item_id))
                .collect(),
            span: stage.span,
        }
    }

    pub(super) fn lower_field_init(
        &mut self,
        field: &FieldInit,
        scope: ScopeId,
        item_id: HirItemId,
    ) -> HirFieldInit {
        match field {
            FieldInit::Shorthand(name) => HirFieldInit::Shorthand {
                name: name.text.clone(),
                resolution: self.resolve_name(&name.text, name.span, scope, true),
                span: name.span,
            },
            FieldInit::Named { name, value } => HirFieldInit::Named {
                name: name.text.clone(),
                value: self.lower_expr(value, scope, item_id),
                span: name.span.cover(value.span()),
            },
        }
    }

    pub(super) fn lower_arg(&mut self, arg: &Arg, scope: ScopeId, item_id: HirItemId) -> HirArg {
        match arg {
            Arg::Positional(expr) => HirArg::Positional(self.lower_expr(expr, scope, item_id)),
            Arg::Named { name, value } => HirArg::Named {
                name: name.text.clone(),
                value: self.lower_expr(value, scope, item_id),
                span: name.span.cover(value.span()),
            },
        }
    }

    pub(super) fn lower_match_arm(
        &mut self,
        arm: &ast::MatchArm,
        parent_scope: ScopeId,
        item_id: HirItemId,
        owner: HirExprId,
    ) -> HirMatchArm {
        let scope = self
            .scopes
            .alloc(Some(parent_scope), ScopeOwner::MatchArm(owner), arm.span);
        let previous_binding = self.pattern_binding;
        self.pattern_binding = Some(super::program::PatternBindingContext {
            owner: PatternBindingOwner::MatchArm { expr: owner },
            ty: None,
            initializer: None,
        });
        let pat = self.lower_pattern(&arm.pattern, scope, item_id);
        self.pattern_binding = previous_binding;
        let body = match &arm.body {
            MatchArmBody::Expr(expr) => {
                HirMatchArmBody::Expr(self.lower_expr(expr, scope, item_id))
            }
            MatchArmBody::Block(block) => {
                HirMatchArmBody::Block(self.lower_block(block, scope, item_id))
            }
        };
        HirMatchArm {
            pat,
            body,
            scope,
            span: arm.span,
        }
    }
}

fn logical_chain_op(op: BinaryOp) -> bool {
    matches!(op, BinaryOp::AndAnd | BinaryOp::OrOr)
}
