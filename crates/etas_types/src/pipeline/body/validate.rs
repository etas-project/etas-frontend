use etas_core::Diagnostic;

use crate::{SolverFailure, ValidationRequest};

use super::state::BodyPipelineState;

pub fn run(ctx: &mut crate::pipeline::context::TypePipelineContext<'_>, state: &BodyPipelineState) {
    for failure in &state.solver_report.failures {
        ctx.diagnostics.push(diagnostic_for_solver_failure(failure));
    }

    for validation in &state.validations {
        match validation {
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
