use etas_core::Diagnostic;

use crate::{SolverFailure, ValidationRequest};

use super::state::BodyPipelineState;
mod coverage;
mod tool_reference;
mod writable_place;

pub fn run(ctx: &mut crate::pipeline::context::TypePipelineContext<'_>, state: &BodyPipelineState) {
    tool_reference::validate(ctx, state);
    for failure in &state.solver_report.failures {
        ctx.diagnostics.push(diagnostic_for_solver_failure(failure));
    }

    for validation in &state.validations {
        match validation {
            ValidationRequest::AgentContext { ty, active, span } => {
                let resolved = crate::substitute_type_params(
                    &mut ctx.interner,
                    *ty,
                    &Default::default(),
                    &state.solver_report.substitutions,
                );
                let valid = resolved
                    .as_ref()
                    .is_ok_and(|ty| match ctx.interner.store().get(*ty) {
                        Some(crate::Type::AgentContext(_)) => active.is_some(),
                        Some(crate::Type::Named(_)) => {
                            state.owner_params.iter().any(|param| param.subject == *ty)
                                || ctx.item_signature(state.item).is_some_and(|signature| {
                                    let callable = match signature {
                                        crate::ItemSignature::Flow(callable)
                                        | crate::ItemSignature::Tool(callable) => Some(callable),
                                        crate::ItemSignature::Agent(agent) => agent.callable(),
                                        crate::ItemSignature::TopLevelLet(_) => None,
                                    };
                                    callable.is_some_and(|callable| {
                                        callable
                                            .generic_params
                                            .iter()
                                            .any(|param| param.subject == *ty)
                                    })
                                })
                        }
                        _ => false,
                    });
                if !valid {
                    ctx.diagnostics.push(Diagnostic::type_check(
                        etas_core::TypeDiagnosticCode::TypeMismatch, *span,
                        "step/invoke requires the current checked agent method or an inherited inference request context",
                    ));
                }
            }
            ValidationRequest::LambdaParameter { ty, span }
            | ValidationRequest::EmptySequence { ty, span }
            | ValidationRequest::EmptyBrace { ty, span } => {
                let resolved = crate::substitute_type_params(
                    &mut ctx.interner,
                    *ty,
                    &Default::default(),
                    &state.solver_report.substitutions,
                );
                match resolved {
                    Ok(ty) => {
                        let (valid, code, message) = match validation {
                            ValidationRequest::LambdaParameter { .. } => (
                                !matches!(ctx.interner.store().get(ty), Some(crate::Type::Var(_))),
                                etas_core::TypeDiagnosticCode::UnknownType,
                                "lambda parameter requires a type annotation or contextual function type",
                            ),
                            ValidationRequest::EmptySequence { .. } => (
                                matches!(
                                    ctx.interner.store().get(ty),
                                    Some(crate::Type::Array(_) | crate::Type::List(_))
                                ),
                                etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                                "empty sequence literal requires an expected Array[T] or List[T] type",
                            ),
                            ValidationRequest::EmptyBrace { .. } => (
                                matches!(
                                    ctx.interner.store().get(ty),
                                    Some(crate::Type::Map { .. })
                                ) || matches!(ctx.interner.store().get(ty), Some(crate::Type::Record(record)) if record.fields.is_empty()),
                                etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                                "empty brace literal requires an expected record or Map[K, V] type",
                            ),
                            _ => unreachable!(),
                        };
                        if !valid {
                            ctx.diagnostics
                                .push(Diagnostic::type_check(code, *span, message));
                        }
                    }
                    Err(error) => ctx.diagnostics.push(Diagnostic::type_check(
                        etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                        *span,
                        format!("contextual type could not be resolved: {error}"),
                    )),
                }
            }
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
