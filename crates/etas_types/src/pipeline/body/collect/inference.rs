use etas_core::{Span, TypeDiagnosticCode};
use etas_hir::{HirArg, HirExprId, HirGenericArg, ResolveResult, ResolvedActionRef};

use crate::{
    ConstraintOrigin, Type, TypeConstraint, TypeId, ValidationRequest,
    pipeline::context::BodyCollectContext,
};

pub(super) fn collect_inference<E>(
    ctx: &mut BodyCollectContext<'_, '_, E>,
    expr: HirExprId,
    action: &ResolvedActionRef,
    generic_args: &[HirGenericArg],
    args: &[HirArg],
    span: Span,
    expected: Option<TypeId>,
) -> Result<TypeId, E> {
    ctx.checkpoint()?;
    let output = match generic_args {
        [] => expected.unwrap_or_else(|| ctx.fresh_type_var()),
        [HirGenericArg::Type(ty)] => crate::lower::type_ref::lower_type_ref(ctx.ctx, *ty)
            .unwrap_or_else(|| ctx.fresh_type_var()),
        _ => {
            ctx.state.validations.push(ValidationRequest::Diagnostic {
                code: TypeDiagnosticCode::TypeMismatch, span,
                message: "perform infer accepts at most one output type argument, not effect rows or static selectors".into(),
            });
            ctx.fresh_type_var()
        }
    };
    let prompt_expected = ctx.ctx.interner.intern(Type::Prompt);
    let mut prompt = None;
    for arg in args {
        let (value, valid_name) = match arg {
            HirArg::Positional(value) => (*value, true),
            HirArg::Named { name, value, .. } => (*value, name == "prompt"),
        };
        let ty = super::expr::collect_expr(ctx, value, Some(prompt_expected))?;
        if args.len() == 1 && valid_name {
            prompt = Some((value, ty));
        }
    }
    let Some((prompt, prompt_type)) = prompt else {
        ctx.state.validations.push(ValidationRequest::Diagnostic {
            code: TypeDiagnosticCode::TypeMismatch,
            span,
            message: "perform infer requires exactly one Prompt argument".into(),
        });
        return Ok(output);
    };
    let ResolveResult::Resolved(action_symbol) = action.action_symbol else {
        ctx.state.validations.push(ValidationRequest::Diagnostic {
            code: TypeDiagnosticCode::IncompleteTypeFacts,
            span,
            message: "agent inference requires its canonical compiler action binding".into(),
        });
        return Ok(output);
    };
    let schema = ctx.ctx.interner.intern(Type::Schema(output));
    ctx.emit(TypeConstraint::AgentInference {
        expr,
        owner: ctx.state.item,
        prompt,
        prompt_type,
        output,
        schema,
        action_symbol,
        origin: ConstraintOrigin { span },
    });
    Ok(output)
}
