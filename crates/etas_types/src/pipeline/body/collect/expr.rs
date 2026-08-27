use etas_hir::{HirBinaryOp, HirElseBranch, HirExpr, ResolveResult};

use crate::{
    AssignabilityReason, ConstraintOrigin, FlowType, MemoryPlaceType, NamedTypeRef, PrimitiveType,
    SymbolTypeFact, Type, TypeConstraint, TypeId,
    pipeline::{
        body::collect::{
            block::collect_block,
            call::collect_call,
            handler::collect_handler,
            index::{collect_index, collect_slice},
            lambda::collect_lambda,
            literal::{collect_literal, collect_negated_numeric_literal},
            method_call::collect_method_call,
            perform::collect_perform,
            record::collect_record,
            spec_method_call::{SpecMethodCallInput, collect_spec_method_call},
            std_member::{
                std_declared_field_type, std_member_value_type, std_member_value_type_for_symbol,
                std_qualified_path_value_type,
            },
        },
        context::BodyCollectContext,
    },
};

pub fn collect_expr(
    ctx: &mut BodyCollectContext<'_, '_>,
    expr: etas_hir::HirExprId,
    expected: Option<TypeId>,
) -> TypeId {
    if let Some(ty) = ctx.state.provisional.expr_types.get(&expr).copied() {
        return ty;
    }
    let ty = match ctx.ctx.hir.exprs[expr].clone() {
        HirExpr::Literal(literal) => collect_literal(ctx, Some(expr), &literal, expected),
        HirExpr::Path(path) => {
            let ty = collect_path(ctx, &path);
            record_path_memory_place(ctx, expr, &path, ty);
            ty
        }
        HirExpr::Tuple { elems, .. } => {
            let elems = elems
                .into_iter()
                .map(|elem| collect_expr(ctx, elem, None))
                .collect();
            ctx.ctx.interner.intern(Type::Tuple(elems))
        }
        HirExpr::Record(record) => collect_record(ctx, &record, expected),
        HirExpr::EmptyRecordOrMap { span } => expected.unwrap_or_else(|| {
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span,
                message: "empty brace literal requires an expected record or Map[K, V] type"
                    .to_owned(),
            });
            ctx.primitive(PrimitiveType::Never)
        }),
        HirExpr::Array { elems, span } => {
            collect_sequence(ctx, elems, span, SequenceKind::Array, expected)
        }
        HirExpr::List { elems, span } => {
            collect_sequence(ctx, elems, span, SequenceKind::List, expected)
        }
        HirExpr::EmptySequence { span } => expected.unwrap_or_else(|| {
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span,
                message: "empty sequence literal requires an expected Array[T] or List[T] type"
                    .to_owned(),
            });
            ctx.primitive(PrimitiveType::Never)
        }),
        HirExpr::Map { entries, span: _ } => {
            let expected_entries = expected.and_then(|ty| match ctx.ctx.interner.store().get(ty) {
                Some(Type::Map { key, value }) => Some((*key, *value)),
                _ => None,
            });
            let mut entries = entries.into_iter();
            let (key, value) = match (expected_entries, entries.next()) {
                (Some(types), first) => {
                    if let Some(first) = first {
                        collect_map_entry(ctx, &first, types.0, types.1);
                    }
                    types
                }
                (None, Some(first)) => {
                    let key = collect_expr(ctx, first.key, None);
                    let value = collect_expr(ctx, first.value, None);
                    (key, value)
                }
                (None, None) => (ctx.fresh_type_var(), ctx.fresh_type_var()),
            };
            for entry in entries {
                collect_map_entry(ctx, &entry, key, value);
            }
            ctx.ctx.interner.intern(Type::Map { key, value })
        }
        HirExpr::Set { elems, span } => {
            if elems.is_empty() {
                expected
                    .and_then(|ty| match ctx.ctx.interner.store().get(ty) {
                        Some(Type::Set(inner)) => Some(ctx.ctx.interner.intern(Type::Set(*inner))),
                        _ => None,
                    })
                    .unwrap_or_else(|| {
                        ctx.validate(crate::ValidationRequest::Diagnostic {
                            code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                            span,
                            message: "empty set literal requires an expected Set[T] type"
                                .to_owned(),
                        });
                        ctx.primitive(PrimitiveType::Never)
                    })
            } else {
                let expected_inner =
                    expected.and_then(|ty| match ctx.ctx.interner.store().get(ty) {
                        Some(Type::Set(inner)) => Some(*inner),
                        _ => None,
                    });
                let mut elems = elems.into_iter();
                let inner = match (expected_inner, elems.next()) {
                    (Some(inner), first) => {
                        if let Some(first) = first {
                            let actual = collect_expr(ctx, first, Some(inner));
                            ctx.emit(TypeConstraint::Assignable {
                                from: actual,
                                to: inner,
                                origin: ConstraintOrigin { span },
                                reason: AssignabilityReason::Other,
                            });
                        }
                        inner
                    }
                    (None, Some(first)) => collect_expr(ctx, first, None),
                    (None, None) => {
                        ctx.validate(crate::ValidationRequest::Diagnostic {
                            code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                            span,
                            message: "set literal is missing its element type".to_owned(),
                        });
                        ctx.fresh_type_var()
                    }
                };
                for elem in elems {
                    let actual = collect_expr(ctx, elem, Some(inner));
                    ctx.emit(TypeConstraint::Assignable {
                        from: actual,
                        to: inner,
                        origin: ConstraintOrigin { span },
                        reason: AssignabilityReason::Other,
                    });
                }
                ctx.ctx.interner.intern(Type::Set(inner))
            }
        }
        HirExpr::Range {
            start, end, span, ..
        } => {
            let start_ty = collect_expr(ctx, start, None);
            let end_ty = collect_expr(ctx, end, Some(start_ty));
            let index_trait = ctx.ctx.interner.intern(Type::Named(NamedTypeRef {
                name: "Index".to_owned(),
            }));
            ctx.emit(TypeConstraint::Assignable {
                from: start_ty,
                to: index_trait,
                origin: ConstraintOrigin { span },
                reason: AssignabilityReason::Other,
            });
            ctx.emit(TypeConstraint::Assignable {
                from: end_ty,
                to: start_ty,
                origin: ConstraintOrigin { span },
                reason: AssignabilityReason::Other,
            });
            ctx.emit(TypeConstraint::Assignable {
                from: end_ty,
                to: index_trait,
                origin: ConstraintOrigin { span },
                reason: AssignabilityReason::Other,
            });
            ctx.ctx.interner.intern(Type::Range { index: start_ty })
        }
        HirExpr::Block(block) => collect_block(ctx, block, expected),
        HirExpr::Call {
            callee,
            generic_args,
            args,
            span,
        } => collect_call(ctx, expr, callee, &generic_args, &args, span, expected),
        HirExpr::Field { base, field, span } => {
            if let Some(ty) = std_member_value_type(ctx, base, &field) {
                ty
            } else {
                let base_ty = collect_expr(ctx, base, None);
                let output = expected.unwrap_or_else(|| ctx.fresh_type_var());
                record_field_memory_place(ctx, expr, base, &field);
                if let Some(field_ty) = std_declared_field_type(ctx, base_ty, &field, span) {
                    ctx.emit(TypeConstraint::Assignable {
                        from: field_ty,
                        to: output,
                        origin: ConstraintOrigin { span },
                        reason: AssignabilityReason::Other,
                    });
                    field_ty
                } else {
                    ctx.emit(TypeConstraint::FieldAccess {
                        base: base_ty,
                        field,
                        output,
                        origin: ConstraintOrigin { span },
                    });
                    output
                }
            }
        }
        HirExpr::Index { base, index, span } => {
            collect_index(ctx, expr, base, index, span, expected)
        }
        HirExpr::Slice {
            base,
            start,
            end,
            span,
            ..
        } => collect_slice(ctx, expr, base, start, end, span, expected),
        HirExpr::Try {
            expr: operand,
            span,
        } => {
            let (output, error) = expected
                .and_then(|ty| match ctx.ctx.interner.store().get(ty) {
                    Some(Type::Result { ok, err }) => Some((*ok, *err)),
                    _ => None,
                })
                .unwrap_or_else(|| (ctx.fresh_type_var(), ctx.fresh_type_var()));
            let operand_ty = collect_expr(ctx, operand, Some(output));
            ctx.emit(TypeConstraint::TryOperand {
                operand: operand_ty,
                output,
                error,
                origin: ConstraintOrigin { span },
            });
            let result = ctx.ctx.interner.intern(Type::Result {
                ok: output,
                err: error,
            });
            ctx.state.provisional.record_try_expr(
                expr,
                crate::TryExprTypeFact {
                    operand,
                    value_type: output,
                    result_type: result,
                    target_error: Some(error),
                },
            );
            result
        }
        HirExpr::If {
            cond,
            then_block,
            else_branch,
            span,
        } => {
            let bool_ty = ctx.primitive(PrimitiveType::Bool);
            let cond_ty = collect_expr(ctx, cond, Some(bool_ty));
            ctx.emit(TypeConstraint::Assignable {
                from: cond_ty,
                to: bool_ty,
                origin: ConstraintOrigin { span },
                reason: AssignabilityReason::Other,
            });
            let branch_expected = expected;
            let then_ty = collect_block(ctx, then_block, branch_expected);
            let else_ty = match else_branch {
                Some(HirElseBranch::If(expr)) => collect_expr(ctx, expr, branch_expected),
                Some(HirElseBranch::Block(block)) => collect_block(ctx, block, branch_expected),
                None => ctx.primitive(PrimitiveType::Unit),
            };
            let result = branch_result_type(ctx, expected, [then_ty, else_ty]);
            emit_branch_assignability(ctx, then_ty, result, span);
            emit_branch_assignability(ctx, else_ty, result, span);
            result
        }
        HirExpr::Binary { op, lhs, rhs, span } => collect_binary(ctx, op, lhs, rhs, span),
        HirExpr::Unary { op, expr, span } => {
            let operand_expected = match op {
                etas_hir::HirUnaryOp::Not => Some(ctx.primitive(PrimitiveType::Bool)),
                etas_hir::HirUnaryOp::Neg => expected,
            };
            let operand = if op == etas_hir::HirUnaryOp::Neg {
                match ctx.ctx.hir.exprs[expr].clone() {
                    HirExpr::Literal(literal) => {
                        collect_negated_numeric_literal(ctx, expr, &literal, operand_expected, span)
                            .unwrap_or_else(|| collect_expr(ctx, expr, operand_expected))
                    }
                    _ => collect_expr(ctx, expr, operand_expected),
                }
            } else {
                collect_expr(ctx, expr, operand_expected)
            };
            let output = match op {
                etas_hir::HirUnaryOp::Not => ctx.primitive(PrimitiveType::Bool),
                _ => expected.unwrap_or(operand),
            };
            ctx.emit(TypeConstraint::Unary {
                op,
                operand,
                output,
                origin: ConstraintOrigin { span },
            });
            output
        }
        HirExpr::Handler { handlers, span } => collect_handler(ctx, &handlers, span, expected),
        HirExpr::Perform {
            action,
            generic_args,
            args,
            span,
        } => collect_perform(ctx, expr, &action, &generic_args, &args, span, expected),
        HirExpr::MethodCall {
            receiver,
            method,
            generic_args,
            args,
            span,
        } => collect_method_call(ctx, receiver, &method, &generic_args, &args, span, expected),
        HirExpr::SpecMethodCall {
            receiver,
            spec_path,
            spec_args,
            method,
            args,
            span,
        } => collect_spec_method_call(
            ctx,
            SpecMethodCallInput {
                receiver,
                spec_path: &spec_path,
                spec_args: &spec_args,
                method: &method,
                args: &args,
                span,
                expected,
            },
        ),
        HirExpr::Handle {
            body,
            handler,
            span,
        } => {
            let handler_ty = collect_expr(ctx, handler, None);
            if !matches!(
                ctx.ctx.interner.store().get(handler_ty),
                Some(Type::Handler(_))
            ) {
                ctx.validate(crate::ValidationRequest::Diagnostic {
                    code: etas_core::TypeDiagnosticCode::TypeMismatch,
                    span,
                    message: "handle expects a handler value".to_owned(),
                });
            }
            collect_expr(ctx, body, expected)
        }
        HirExpr::StageCompose { stages, span } => {
            collect_stage_composition(ctx, stages, span, expected)
        }
        HirExpr::Pipeline {
            input,
            stages,
            span,
        } => collect_pipeline(ctx, input, stages, span, expected),
        HirExpr::Lambda {
            params, body, span, ..
        } => collect_lambda(ctx, params, body, span, expected),
        HirExpr::ListCons { head, tail, span } => {
            collect_list_cons(ctx, head, tail, span, expected)
        }
        HirExpr::Match {
            scrutinee,
            arms,
            span,
        } => collect_match(ctx, scrutinee, arms, span, expected),
        HirExpr::Error { span } => {
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span,
                message: "error expression cannot be typed".to_owned(),
            });
            ctx.primitive(PrimitiveType::Never)
        }
    };
    ctx.record_expr_type(expr, ty)
}

pub fn collect_if_statement(
    ctx: &mut BodyCollectContext<'_, '_>,
    cond: etas_hir::HirExprId,
    then_block: etas_hir::HirBlockId,
    else_branch: Option<HirElseBranch>,
    span: etas_core::Span,
) {
    let bool_ty = ctx.primitive(PrimitiveType::Bool);
    let cond_ty = collect_expr(ctx, cond, Some(bool_ty));
    ctx.emit(TypeConstraint::Assignable {
        from: cond_ty,
        to: bool_ty,
        origin: ConstraintOrigin { span },
        reason: AssignabilityReason::Other,
    });
    collect_block(ctx, then_block, None);
    match else_branch {
        Some(HirElseBranch::If(expr)) => {
            collect_expr(ctx, expr, None);
        }
        Some(HirElseBranch::Block(block)) => {
            collect_block(ctx, block, None);
        }
        None => {}
    }
}

pub fn collect_match_statement(
    ctx: &mut BodyCollectContext<'_, '_>,
    scrutinee: etas_hir::HirExprId,
    arms: Vec<etas_hir::HirMatchArm>,
) {
    let scrutinee_ty = collect_expr(ctx, scrutinee, None);
    let mut arm_types = Vec::new();
    for arm in arms {
        crate::pipeline::body::collect::pattern::collect_pattern(ctx, arm.pat, scrutinee_ty);
        let arm_ty = match arm.body {
            etas_hir::HirMatchArmBody::Expr(expr) => collect_expr(ctx, expr, None),
            etas_hir::HirMatchArmBody::Block(block) => collect_block(ctx, block, None),
        };
        arm_types.push((arm_ty, arm.span));
    }
    let result = branch_result_type(ctx, None, arm_types.iter().map(|(ty, _)| *ty));
    for (arm_ty, arm_span) in arm_types {
        emit_branch_assignability(ctx, arm_ty, result, arm_span);
    }
}

fn collect_list_cons(
    ctx: &mut BodyCollectContext<'_, '_>,
    head: etas_hir::HirExprId,
    tail: etas_hir::HirExprId,
    span: etas_core::Span,
    expected: Option<TypeId>,
) -> TypeId {
    let inner = expected
        .and_then(|ty| match ctx.ctx.interner.store().get(ty) {
            Some(Type::List(inner)) => Some(*inner),
            _ => None,
        })
        .unwrap_or_else(|| ctx.fresh_type_var());
    let list = ctx.ctx.interner.intern(Type::List(inner));
    let head_ty = collect_expr(ctx, head, Some(inner));
    let tail_ty = collect_expr(ctx, tail, Some(list));
    ctx.emit(TypeConstraint::Assignable {
        from: head_ty,
        to: inner,
        origin: ConstraintOrigin { span },
        reason: AssignabilityReason::Other,
    });
    ctx.emit(TypeConstraint::Assignable {
        from: tail_ty,
        to: list,
        origin: ConstraintOrigin { span },
        reason: AssignabilityReason::Other,
    });
    list
}

fn collect_match(
    ctx: &mut BodyCollectContext<'_, '_>,
    scrutinee: etas_hir::HirExprId,
    arms: Vec<etas_hir::HirMatchArm>,
    span: etas_core::Span,
    expected: Option<TypeId>,
) -> TypeId {
    let scrutinee_ty = collect_expr(ctx, scrutinee, None);
    let branch_expected = expected;
    let mut arm_types = Vec::new();
    for arm in arms {
        crate::pipeline::body::collect::pattern::collect_pattern(ctx, arm.pat, scrutinee_ty);
        let arm_ty = match arm.body {
            etas_hir::HirMatchArmBody::Expr(expr) => collect_expr(ctx, expr, branch_expected),
            etas_hir::HirMatchArmBody::Block(block) => collect_block(ctx, block, branch_expected),
        };
        arm_types.push((arm_ty, arm.span));
    }
    let result = branch_result_type(ctx, expected, arm_types.iter().map(|(ty, _)| *ty));
    for (arm_ty, arm_span) in arm_types {
        emit_branch_assignability(ctx, arm_ty, result, arm_span);
    }
    if ctx.ctx.hir.exprs[scrutinee]
        .span(&ctx.ctx.hir.blocks)
        .source
        != span.source
    {
        return result;
    }
    result
}

fn branch_result_type(
    ctx: &mut BodyCollectContext<'_, '_>,
    expected: Option<TypeId>,
    branch_types: impl IntoIterator<Item = TypeId>,
) -> TypeId {
    if let Some(expected) = expected {
        return expected;
    }
    branch_types
        .into_iter()
        .find(|ty| !is_never_type(ctx, *ty))
        .unwrap_or_else(|| ctx.primitive(PrimitiveType::Never))
}

fn emit_branch_assignability(
    ctx: &mut BodyCollectContext<'_, '_>,
    from: TypeId,
    to: TypeId,
    span: etas_core::Span,
) {
    ctx.emit(TypeConstraint::Assignable {
        from,
        to,
        origin: ConstraintOrigin { span },
        reason: AssignabilityReason::Branch,
    });
}

fn is_never_type(ctx: &BodyCollectContext<'_, '_>, ty: TypeId) -> bool {
    matches!(
        ctx.ctx.interner.store().get(ty),
        Some(Type::Primitive(PrimitiveType::Never))
    )
}

fn collect_stage_composition(
    ctx: &mut BodyCollectContext<'_, '_>,
    stages: Vec<etas_hir::HirStage>,
    span: etas_core::Span,
    expected: Option<TypeId>,
) -> TypeId {
    let expected_flow = expected.and_then(|ty| match ctx.ctx.interner.store().get(ty) {
        Some(Type::Function(flow)) => Some(flow.clone()),
        _ => None,
    });
    let input = expected_flow
        .as_ref()
        .and_then(|flow| flow.input.first().copied())
        .unwrap_or_else(|| ctx.fresh_type_var());
    let mut current = input;
    for stage in stages {
        for limit in stage.limits {
            collect_expr(ctx, limit, None);
        }
        let stage_ty = collect_expr(ctx, stage.expr, None);
        let output = ctx.fresh_type_var();
        ctx.emit(TypeConstraint::Callable {
            call: None,
            callee: stage_ty,
            generic_params: Vec::new(),
            generic_args: Vec::new(),
            args: vec![current],
            output,
            origin: ConstraintOrigin { span: stage.span },
        });
        current = output;
    }
    let output = expected_flow.map(|flow| flow.output).unwrap_or(current);
    if output != current {
        ctx.emit(TypeConstraint::Assignable {
            from: current,
            to: output,
            origin: ConstraintOrigin { span },
            reason: AssignabilityReason::Other,
        });
    }
    ctx.ctx.interner.intern(Type::Function(FlowType {
        input: vec![input],
        output,
        effects: None,
    }))
}

fn collect_pipeline(
    ctx: &mut BodyCollectContext<'_, '_>,
    input: etas_hir::HirExprId,
    stages: Vec<etas_hir::HirStage>,
    span: etas_core::Span,
    expected: Option<TypeId>,
) -> TypeId {
    let mut current = collect_expr(ctx, input, None);
    for stage in stages {
        for limit in stage.limits {
            collect_expr(ctx, limit, None);
        }
        let stage_ty = collect_expr(ctx, stage.expr, None);
        let output = ctx.fresh_type_var();
        ctx.emit(TypeConstraint::Callable {
            call: None,
            callee: stage_ty,
            generic_params: Vec::new(),
            generic_args: Vec::new(),
            args: vec![current],
            output,
            origin: ConstraintOrigin { span: stage.span },
        });
        current = output;
    }
    if let Some(expected) = expected {
        ctx.emit(TypeConstraint::Assignable {
            from: current,
            to: expected,
            origin: ConstraintOrigin { span },
            reason: AssignabilityReason::Other,
        });
        expected
    } else {
        current
    }
}

fn collect_path(ctx: &mut BodyCollectContext<'_, '_>, path: &etas_hir::ResolvedPath) -> TypeId {
    match path.resolution {
        ResolveResult::Resolved(symbol) => symbol_value_type(ctx, symbol).unwrap_or_else(|| {
            if let Some(ty) = std_qualified_path_value_type(ctx, path) {
                return ty;
            }
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::UnknownType,
                span: path.span,
                message: "resolved symbol has no type fact".to_owned(),
            });
            ctx.primitive(PrimitiveType::Never)
        }),
        ResolveResult::PartiallyResolved(ref partial) if partial.resolved_prefix.is_some() => {
            let symbol = partial.resolved_prefix.expect("checked is_some");
            if partial.remaining.len() == 1
                && let Some(ty) =
                    std_member_value_type_for_symbol(ctx, symbol, &partial.remaining[0])
            {
                return ty;
            }
            if let Some(mut current) = symbol_value_type(ctx, symbol) {
                for member in &partial.remaining {
                    if let Some(field_ty) = std_declared_field_type(ctx, current, member, path.span)
                    {
                        current = field_ty;
                        continue;
                    }
                    let output = ctx.fresh_type_var();
                    ctx.emit(TypeConstraint::FieldAccess {
                        base: current,
                        field: member.clone(),
                        output,
                        origin: ConstraintOrigin { span: path.span },
                    });
                    current = output;
                }
                current
            } else {
                let never = ctx.primitive(PrimitiveType::Never);
                ctx.emit(TypeConstraint::FieldAccess {
                    base: never,
                    field: partial.remaining.join("."),
                    output: never,
                    origin: ConstraintOrigin { span: path.span },
                });
                ctx.validate(crate::ValidationRequest::Diagnostic {
                    code: etas_core::TypeDiagnosticCode::UnknownType,
                    span: path.span,
                    message: "path could not be resolved to a typed value".to_owned(),
                });
                ctx.primitive(PrimitiveType::Never)
            }
        }
        _ => {
            if let Some(ty) = std_qualified_path_value_type(ctx, path) {
                return ty;
            }
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::UnknownType,
                span: path.span,
                message: "path could not be resolved to a typed value".to_owned(),
            });
            ctx.primitive(PrimitiveType::Never)
        }
    }
}

fn record_path_memory_place(
    ctx: &mut BodyCollectContext<'_, '_>,
    expr: etas_hir::HirExprId,
    path: &etas_hir::ResolvedPath,
    ty: TypeId,
) {
    match &path.resolution {
        ResolveResult::Resolved(symbol) => {
            if !is_memory_place_root_type(ctx, ty) {
                return;
            }
            let Some(segments) = canonical_symbol_path(ctx, *symbol) else {
                return;
            };
            let place = ctx
                .ctx
                .interner
                .intern(Type::MemoryPlace(MemoryPlaceType { segments }));
            ctx.record_expr_memory_place(expr, place);
        }
        ResolveResult::PartiallyResolved(partial) => {
            let Some(symbol) = partial.resolved_prefix else {
                return;
            };
            let Some(base_ty) = symbol_value_type(ctx, symbol) else {
                return;
            };
            if !is_memory_place_root_type(ctx, base_ty) {
                return;
            }
            let Some(mut segments) = canonical_symbol_path(ctx, symbol) else {
                return;
            };
            segments.extend(partial.remaining.iter().cloned());
            let place = ctx
                .ctx
                .interner
                .intern(Type::MemoryPlace(MemoryPlaceType { segments }));
            ctx.record_expr_memory_place(expr, place);
        }
        _ => {}
    }
}

fn canonical_symbol_path(
    ctx: &BodyCollectContext<'_, '_>,
    symbol: etas_hir::SymbolId,
) -> Option<Vec<String>> {
    let symbol = ctx.ctx.hir.symbols.get(symbol)?;
    if let etas_hir::SymbolDef::ImportAlias { path, .. } = &symbol.def {
        return Some(path.clone());
    }
    let module = ctx.ctx.hir.modules_arena.get(symbol.defining_module)?;
    let mut segments = module
        .name
        .as_ref()
        .map(|module_name| {
            module_name
                .segments
                .iter()
                .map(|segment| segment.name.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    segments.push(symbol.name.clone());
    Some(segments)
}

fn record_field_memory_place(
    ctx: &mut BodyCollectContext<'_, '_>,
    expr: etas_hir::HirExprId,
    base: etas_hir::HirExprId,
    field: &str,
) {
    let Some(base_place) = ctx
        .state
        .provisional
        .expr_memory_places
        .get(&base)
        .or_else(|| ctx.ctx.signature_facts.expr_memory_places.get(&base))
        .copied()
    else {
        return;
    };
    let Some(Type::MemoryPlace(base_place)) = ctx.ctx.interner.store().get(base_place).cloned()
    else {
        return;
    };
    let mut segments = base_place.segments;
    segments.push(field.to_owned());
    let place = ctx
        .ctx
        .interner
        .intern(Type::MemoryPlace(MemoryPlaceType { segments }));
    ctx.record_expr_memory_place(expr, place);
}

fn is_memory_place_root_type(ctx: &BodyCollectContext<'_, '_>, ty: TypeId) -> bool {
    matches!(
        ctx.ctx.interner.store().get(ty),
        Some(Type::MemoryRegion(_))
            | Some(Type::ResourceHandle(
                crate::ResourceHandleType::MemoryRegion { .. }
            ))
    )
}

pub fn symbol_value_type(
    ctx: &mut BodyCollectContext<'_, '_>,
    symbol: etas_hir::SymbolId,
) -> Option<TypeId> {
    let fact = ctx
        .state
        .provisional
        .symbol_types
        .get(&symbol)
        .or_else(|| {
            ctx.ctx
                .symbols
                .symbol_fact(ctx.ctx.hir, &ctx.ctx.signature_facts, symbol)
        })?
        .clone();
    value_type_from_fact(ctx, fact)
}

pub fn value_type_from_fact(
    ctx: &mut BodyCollectContext<'_, '_>,
    fact: SymbolTypeFact,
) -> Option<TypeId> {
    value_type_from_fact_with_instantiation(ctx, fact, true)
}

pub fn raw_value_type_from_fact(
    ctx: &mut BodyCollectContext<'_, '_>,
    fact: SymbolTypeFact,
) -> Option<TypeId> {
    value_type_from_fact_with_instantiation(ctx, fact, false)
}

pub fn callable_candidate_from_fact(
    ctx: &mut BodyCollectContext<'_, '_>,
    fact: SymbolTypeFact,
    instantiate_schematics: bool,
) -> Option<crate::CallableCandidate> {
    let signature = match fact {
        SymbolTypeFact::Flow { signature }
        | SymbolTypeFact::Agent { signature }
        | SymbolTypeFact::Tool { signature } => signature,
        _ => return None,
    };
    let mut schematic_vars = std::collections::HashMap::new();
    if instantiate_schematics {
        for param in &signature.generic_params {
            schematic_vars
                .entry(param.name.clone())
                .or_insert_with(|| ctx.fresh_type_var());
        }
    }
    let params = if instantiate_schematics {
        signature
            .params
            .into_iter()
            .map(|ty| instantiate_callable_schematic_type(ctx, ty, &mut schematic_vars))
            .collect()
    } else {
        signature.params
    };
    let output = if instantiate_schematics {
        instantiate_callable_schematic_type(ctx, signature.output, &mut schematic_vars)
    } else {
        signature.output
    };
    let generic_params = if instantiate_schematics {
        signature
            .generic_params
            .into_iter()
            .map(|param| crate::CallableGenericParam {
                name: param.name,
                subject: instantiate_callable_schematic_type(
                    ctx,
                    param.subject,
                    &mut schematic_vars,
                ),
                bounds: param
                    .bounds
                    .into_iter()
                    .map(|bound| crate::CheckedSpecBound {
                        spec: bound.spec,
                        args: bound
                            .args
                            .into_iter()
                            .map(|arg| {
                                instantiate_callable_schematic_type(ctx, arg, &mut schematic_vars)
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect()
    } else {
        signature.generic_params
    };
    let ty = ctx.ctx.interner.intern(Type::Function(FlowType {
        input: params,
        output,
        effects: signature.effects,
    }));
    Some(crate::CallableCandidate { ty, generic_params })
}

fn value_type_from_fact_with_instantiation(
    ctx: &mut BodyCollectContext<'_, '_>,
    fact: SymbolTypeFact,
    instantiate_schematics: bool,
) -> Option<TypeId> {
    match fact {
        SymbolTypeFact::Param { ty }
        | SymbolTypeFact::Local { ty, .. }
        | SymbolTypeFact::Field { ty }
        | SymbolTypeFact::Value { ty }
        | SymbolTypeFact::TopLevelLet { ty, .. } => Some(ty),
        SymbolTypeFact::Flow { signature }
        | SymbolTypeFact::Agent { signature }
        | SymbolTypeFact::Tool { signature } => callable_candidate_from_fact(
            ctx,
            SymbolTypeFact::Flow { signature },
            instantiate_schematics,
        )
        .map(|candidate| candidate.ty),
        _ => None,
    }
}

fn instantiate_callable_schematic_type(
    ctx: &mut BodyCollectContext<'_, '_>,
    ty: TypeId,
    schematic_vars: &mut std::collections::HashMap<String, TypeId>,
) -> TypeId {
    let Some(ty_data) = ctx.ctx.interner.store().get(ty).cloned() else {
        return ty;
    };
    match ty_data {
        Type::Named(name) if is_schematic_type_variable_name(&name.name) => *schematic_vars
            .entry(name.name)
            .or_insert_with(|| ctx.fresh_type_var()),
        Type::Array(inner) => {
            let inner = instantiate_callable_schematic_type(ctx, inner, schematic_vars);
            ctx.ctx.interner.intern(Type::Array(inner))
        }
        Type::List(inner) => {
            let inner = instantiate_callable_schematic_type(ctx, inner, schematic_vars);
            ctx.ctx.interner.intern(Type::List(inner))
        }
        Type::Set(inner) => {
            let inner = instantiate_callable_schematic_type(ctx, inner, schematic_vars);
            ctx.ctx.interner.intern(Type::Set(inner))
        }
        Type::Slice(inner) => {
            let inner = instantiate_callable_schematic_type(ctx, inner, schematic_vars);
            ctx.ctx.interner.intern(Type::Slice(inner))
        }
        Type::Option(inner) => {
            let inner = instantiate_callable_schematic_type(ctx, inner, schematic_vars);
            ctx.ctx.interner.intern(Type::Option(inner))
        }
        Type::Result { ok, err } => {
            let ok = instantiate_callable_schematic_type(ctx, ok, schematic_vars);
            let err = instantiate_callable_schematic_type(ctx, err, schematic_vars);
            ctx.ctx.interner.intern(Type::Result { ok, err })
        }
        Type::Map { key, value } => {
            let key = instantiate_callable_schematic_type(ctx, key, schematic_vars);
            let value = instantiate_callable_schematic_type(ctx, value, schematic_vars);
            ctx.ctx.interner.intern(Type::Map { key, value })
        }
        Type::Tuple(elements) => {
            let elements = elements
                .into_iter()
                .map(|ty| instantiate_callable_schematic_type(ctx, ty, schematic_vars))
                .collect();
            ctx.ctx.interner.intern(Type::Tuple(elements))
        }
        Type::Record(record) => {
            let fields = record
                .fields
                .into_iter()
                .map(|field| crate::FieldType {
                    name: field.name,
                    ty: instantiate_callable_schematic_type(ctx, field.ty, schematic_vars),
                })
                .collect();
            ctx.ctx
                .interner
                .intern(Type::Record(crate::RecordType { fields }))
        }
        Type::Trust { wrapper, inner } => {
            let inner = instantiate_callable_schematic_type(ctx, inner, schematic_vars);
            ctx.ctx.interner.intern(Type::Trust { wrapper, inner })
        }
        Type::Schema(inner) => {
            let inner = instantiate_callable_schematic_type(ctx, inner, schematic_vars);
            ctx.ctx.interner.intern(Type::Schema(inner))
        }
        Type::Message(inner) => {
            let inner = instantiate_callable_schematic_type(ctx, inner, schematic_vars);
            ctx.ctx.interner.intern(Type::Message(inner))
        }
        Type::MemorySelection(inner) => {
            let inner = instantiate_callable_schematic_type(ctx, inner, schematic_vars);
            ctx.ctx.interner.intern(Type::MemorySelection(inner))
        }
        Type::Store { key, value } => {
            let key = instantiate_callable_schematic_type(ctx, key, schematic_vars);
            let value = instantiate_callable_schematic_type(ctx, value, schematic_vars);
            ctx.ctx.interner.intern(Type::Store { key, value })
        }
        Type::MemoryRegion(inner) => {
            let inner = instantiate_callable_schematic_type(ctx, inner, schematic_vars);
            ctx.ctx.interner.intern(Type::MemoryRegion(inner))
        }
        Type::ResourceHandle(crate::ResourceHandleType::MemoryRegion { schema }) => {
            let schema = instantiate_callable_schematic_type(ctx, schema, schematic_vars);
            ctx.ctx.interner.intern(Type::ResourceHandle(
                crate::ResourceHandleType::MemoryRegion { schema },
            ))
        }
        Type::ResourceHandle(crate::ResourceHandleType::ExternalTool { signature }) => {
            let signature = instantiate_callable_schematic_type(ctx, signature, schematic_vars);
            ctx.ctx.interner.intern(Type::ResourceHandle(
                crate::ResourceHandleType::ExternalTool { signature },
            ))
        }
        Type::ResourceHandle(crate::ResourceHandleType::Other { name, args }) => {
            let args = args
                .into_iter()
                .map(|ty| instantiate_callable_schematic_type(ctx, ty, schematic_vars))
                .collect();
            ctx.ctx
                .interner
                .intern(Type::ResourceHandle(crate::ResourceHandleType::Other {
                    name,
                    args,
                }))
        }
        Type::Applied { constructor, args } => {
            let args = args
                .into_iter()
                .map(|ty| instantiate_callable_schematic_type(ctx, ty, schematic_vars))
                .collect();
            ctx.ctx.interner.intern(Type::Applied { constructor, args })
        }
        Type::Function(mut flow) => {
            flow.input = flow
                .input
                .into_iter()
                .map(|ty| instantiate_callable_schematic_type(ctx, ty, schematic_vars))
                .collect();
            flow.output = instantiate_callable_schematic_type(ctx, flow.output, schematic_vars);
            ctx.ctx.interner.intern(Type::Function(flow))
        }
        _ => ty,
    }
}

fn is_schematic_type_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(ch) if ch.is_ascii_uppercase()) && chars.next().is_none()
}

#[derive(Clone, Copy)]
enum SequenceKind {
    Array,
    List,
}

fn collect_sequence(
    ctx: &mut BodyCollectContext<'_, '_>,
    elems: Vec<etas_hir::HirExprId>,
    span: etas_core::Span,
    kind: SequenceKind,
    expected: Option<TypeId>,
) -> TypeId {
    let expected_inner = expected.and_then(|ty| match ctx.ctx.interner.store().get(ty) {
        Some(Type::Array(inner)) | Some(Type::List(inner)) => Some(*inner),
        _ => None,
    });
    let mut elems = elems.into_iter();
    let inner = match (expected_inner, elems.next()) {
        (Some(inner), first) => {
            if let Some(first) = first {
                let actual = collect_expr(ctx, first, Some(inner));
                ctx.emit(TypeConstraint::Assignable {
                    from: actual,
                    to: inner,
                    origin: ConstraintOrigin { span },
                    reason: AssignabilityReason::Other,
                });
            }
            inner
        }
        (None, Some(first)) => collect_expr(ctx, first, None),
        (None, None) => {
            ctx.validate(crate::ValidationRequest::Diagnostic {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span,
                message: "sequence literal is missing its element type".to_owned(),
            });
            ctx.fresh_type_var()
        }
    };
    for elem in elems {
        let actual = collect_expr(ctx, elem, Some(inner));
        ctx.emit(TypeConstraint::Assignable {
            from: actual,
            to: inner,
            origin: ConstraintOrigin { span },
            reason: AssignabilityReason::Other,
        });
    }
    match kind {
        SequenceKind::Array => ctx.ctx.interner.intern(Type::Array(inner)),
        SequenceKind::List => ctx.ctx.interner.intern(Type::List(inner)),
    }
}

fn collect_map_entry(
    ctx: &mut BodyCollectContext<'_, '_>,
    entry: &etas_hir::HirMapEntry,
    key: TypeId,
    value: TypeId,
) {
    let actual_key = collect_expr(ctx, entry.key, Some(key));
    let actual_value = collect_expr(ctx, entry.value, Some(value));
    ctx.emit(TypeConstraint::Assignable {
        from: actual_key,
        to: key,
        origin: ConstraintOrigin { span: entry.span },
        reason: AssignabilityReason::Other,
    });
    ctx.emit(TypeConstraint::Assignable {
        from: actual_value,
        to: value,
        origin: ConstraintOrigin { span: entry.span },
        reason: AssignabilityReason::Other,
    });
}

fn collect_binary(
    ctx: &mut BodyCollectContext<'_, '_>,
    op: HirBinaryOp,
    lhs: etas_hir::HirExprId,
    rhs: etas_hir::HirExprId,
    span: etas_core::Span,
) -> TypeId {
    let lhs_ty = collect_expr(ctx, lhs, None);
    let rhs_ty = collect_expr(ctx, rhs, Some(lhs_ty));
    ctx.emit(TypeConstraint::Assignable {
        from: rhs_ty,
        to: lhs_ty,
        origin: ConstraintOrigin { span },
        reason: AssignabilityReason::Other,
    });
    match op {
        HirBinaryOp::AndAnd
        | HirBinaryOp::OrOr
        | HirBinaryOp::EqEq
        | HirBinaryOp::BangEq
        | HirBinaryOp::Lt
        | HirBinaryOp::LtEq
        | HirBinaryOp::Gt
        | HirBinaryOp::GtEq => ctx.primitive(PrimitiveType::Bool),
        _ => lhs_ty,
    }
}
