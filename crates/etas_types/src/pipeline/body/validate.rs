use etas_core::Diagnostic;

use crate::{SolverFailure, ValidationRequest};

use super::state::BodyPipelineState;
mod coverage;
mod writable_place;

pub fn run(ctx: &mut crate::pipeline::context::TypePipelineContext<'_>, state: &BodyPipelineState) {
    for failure in &state.solver_report.failures {
        ctx.diagnostics.push(diagnostic_for_solver_failure(failure));
    }

    for validation in &state.validations {
        match validation {
            ValidationRequest::HandlerNeverCompletion { ty, span } => {
                let ty = crate::substitute_type_params(
                    &mut ctx.interner,
                    *ty,
                    &Default::default(),
                    &state.solver_report.substitutions,
                );
                match ty {
                    Ok(ty)
                        if matches!(
                            ctx.interner.store().get(ty),
                            Some(crate::Type::Primitive(crate::PrimitiveType::Never))
                        ) => {}
                    Ok(_) => ctx.diagnostics.push(Diagnostic::type_check(
                        etas_core::TypeDiagnosticCode::HandlerCompletionRequired,
                        *span,
                        "handler arm must explicitly resume, finish, or terminate with never",
                    )),
                    Err(error) => ctx.diagnostics.push(Diagnostic::type_check(
                        etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                        *span,
                        format!("handler completion type could not be resolved: {error}"),
                    )),
                }
            }
            ValidationRequest::WritablePlace { target, span } => {
                writable_place::validate(ctx, state, *target, *span)
            }
            ValidationRequest::MatchCoverage {
                scrutinee,
                arms,
                span,
            } => {
                coverage::validate(ctx, state, *scrutinee, arms, *span);
            }
            ValidationRequest::CallableArity { .. } => {}
            ValidationRequest::Diagnostic {
                code,
                span,
                message,
            } => {
                ctx.diagnostics
                    .push(Diagnostic::type_check(code.clone(), *span, message.clone()));
            }
        }
    }
}

fn diagnostic_for_solver_failure(failure: &SolverFailure) -> Diagnostic {
    Diagnostic::type_check(failure.code.clone(), failure.span, failure.message.clone())
}
