use crate::{TypeId, method::MethodSelection, pipeline::context::BodyCollectContext};
use etas_hir::{HirArg, ResolveResult, ResolvedPath};

pub struct SpecMethodCallInput<'a> {
    pub expr: etas_hir::HirExprId,
    pub receiver: etas_hir::HirExprId,
    pub spec_path: &'a ResolvedPath,
    pub spec_args: &'a [etas_hir::HirTypeId],
    pub method: &'a str,
    pub generic_args: &'a [etas_hir::HirGenericArg],
    pub args: &'a [HirArg],
    pub span: etas_core::Span,
    pub expected: Option<TypeId>,
}

pub fn collect_spec_method_call<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    input: SpecMethodCallInput<'_>,
) -> Result<TypeId, E> {
    let ResolveResult::Resolved(symbol) = input.spec_path.resolution else {
        ctx.validate(crate::ValidationRequest::Diagnostic {
            code: etas_core::TypeDiagnosticCode::UnknownType,
            span: input.spec_path.span,
            message: "spec method selection must name a resolved spec".into(),
        });
        return Ok(ctx.primitive(crate::PrimitiveType::Never));
    };
    let symbol = ctx
        .ctx
        .symbols
        .canonical_symbol(ctx.ctx.hir, symbol)
        .unwrap_or(symbol);
    let args = input
        .spec_args
        .iter()
        .filter_map(|arg| crate::lower::type_ref::lower_type_ref(ctx.ctx, *arg))
        .collect();
    super::method_call::collect_selected(
        ctx,
        super::method_call::SelectedMethodCall {
            binding: None,
            expr: input.expr,
            receiver: input.receiver,
            method: input.method,
            generic_args: input.generic_args,
            args: input.args,
            selection: MethodSelection::ExplicitSpec {
                spec: crate::CheckedSpecRef::Source(symbol),
                args,
            },
            expected: input.expected,
        },
    )
}
