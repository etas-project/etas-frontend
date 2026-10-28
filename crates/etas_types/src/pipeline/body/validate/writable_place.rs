use etas_core::{Diagnostic, Span, TypeDiagnosticCode};
use etas_hir::{HirExpr, HirExprId};

use crate::pipeline::{body::state::BodyPipelineState, context::TypePipelineContext};

pub(super) fn validate(
    ctx: &mut TypePipelineContext<'_>,
    state: &BodyPipelineState,
    mut target: HirExprId,
    span: Span,
) {
    loop {
        if let Some(projections) = state.provisional.field_projections.get(&target) {
            for projection in projections {
                let receiver = match crate::substitute_type_params(
                    &mut ctx.interner,
                    projection.receiver,
                    &Default::default(),
                    &state.solver_report.substitutions,
                ) {
                    Ok(ty) => ty,
                    Err(error) => {
                        ctx.diagnostics.push(Diagnostic::type_check(
                            TypeDiagnosticCode::IncompleteTypeFacts,
                            span,
                            format!("assignment projection could not be resolved: {error}"),
                        ));
                        return;
                    }
                };
                if ctx
                    .known_std_types()
                    .projected_field_templates(ctx.interner.store(), receiver)
                    .is_some_and(|fields| fields.iter().any(|field| field.name == projection.field))
                {
                    ctx.diagnostics.push(Diagnostic::type_check(
                        TypeDiagnosticCode::TypeMismatch,
                        span,
                        format!("field projection `{}` is read-only", projection.field),
                    ));
                    return;
                }
            }
        }
        match ctx.hir.exprs.get(target) {
            Some(
                HirExpr::Field { base, .. }
                | HirExpr::Index { base, .. }
                | HirExpr::Slice { base, .. },
            ) => target = *base,
            _ => return,
        }
    }
}
