use etas_hir::{HirLambdaBody, SymbolDef};

use crate::{
    FlowType, PrimitiveType, SymbolTypeFact, Type, TypeId,
    lower::type_ref::lower_type_ref,
    pipeline::{
        body::collect::{block::collect_block, expr::collect_expr},
        context::BodyCollectContext,
    },
};

pub fn collect_lambda(
    ctx: &mut BodyCollectContext<'_, '_>,
    params: Vec<etas_hir::SymbolId>,
    body: HirLambdaBody,
    span: etas_core::Span,
    expected: Option<TypeId>,
) -> TypeId {
    let expected_flow = expected.and_then(|ty| match ctx.ctx.interner.store().get(ty) {
        Some(Type::Function(flow)) => Some(flow.clone()),
        _ => None,
    });

    let mut input = Vec::with_capacity(params.len());
    for (index, param) in params.iter().copied().enumerate() {
        let ty = parameter_type(ctx, param)
            .or_else(|| {
                expected_flow
                    .as_ref()
                    .and_then(|flow| flow.input.get(index).copied())
            })
            .unwrap_or_else(|| {
                ctx.validate(crate::ValidationRequest::Diagnostic {
                    code: etas_core::TypeDiagnosticCode::UnknownType,
                    span: ctx
                        .ctx
                        .hir
                        .symbols
                        .get(param)
                        .map(|symbol| symbol.definition_span)
                        .unwrap_or(span),
                    message:
                        "lambda parameter requires a type annotation or contextual function type"
                            .to_owned(),
                });
                ctx.primitive(PrimitiveType::Never)
            });
        ctx.record_symbol_type(param, SymbolTypeFact::Param { ty });
        input.push(ty);
    }

    let output = expected_flow
        .as_ref()
        .map(|flow| flow.output)
        .unwrap_or_else(|| ctx.fresh_type_var());
    let previous_return = ctx.state.expected_return.replace(output);
    let body_ty = match body {
        HirLambdaBody::Expr(expr) => collect_expr(ctx, expr, Some(output)),
        HirLambdaBody::Block(block) => collect_block(ctx, block, Some(output)),
    };
    ctx.state.expected_return = previous_return;
    ctx.emit(crate::TypeConstraint::Assignable {
        from: body_ty,
        to: output,
        origin: crate::ConstraintOrigin { span },
        reason: crate::AssignabilityReason::Return,
    });

    ctx.ctx.interner.intern(Type::Function(FlowType {
        input,
        output,
        effects: expected_flow.and_then(|flow| flow.effects),
    }))
}

fn parameter_type(
    ctx: &mut BodyCollectContext<'_, '_>,
    symbol: etas_hir::SymbolId,
) -> Option<TypeId> {
    match &ctx.ctx.hir.symbols.get(symbol)?.def {
        SymbolDef::Param { ty: Some(ty), .. } => lower_type_ref(ctx.ctx, *ty),
        _ => None,
    }
}
