use std::collections::HashMap;

use etas_hir::HirHandlerArmId;

use crate::{SolverFailure, TypeId, TypeOutput, TypeUnifier};

pub struct HandlerInstantiationProof {
    pub arm: HirHandlerArmId,
    pub requests: Vec<Vec<(String, TypeId)>>,
}

pub fn complete_handler_instantiations(
    output: &mut TypeOutput,
    proofs: &[HandlerInstantiationProof],
) -> Result<(), Vec<SolverFailure>> {
    let mut indexed = HashMap::new();
    let mut failures = Vec::new();
    for proof in proofs {
        indexed.insert(proof.arm, proof);
    }
    for (arm, obligation) in &output.facts.deferred_handler_instantiations {
        let failure = |code, message| SolverFailure {
            code,
            span: obligation.span,
            message,
        };
        let Some(proof) = indexed.get(arm) else {
            failures.push(failure(
                etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                "handler instantiation has no complete checked request proof".into(),
            ));
            continue;
        };
        for parameter in &obligation.parameters {
            if parameter.parametric {
                continue;
            }
            if proof.requests.is_empty() {
                failures.push(failure(
                    etas_core::TypeDiagnosticCode::TypeMismatch,
                    format!(
                        "handler parameter `{}` cannot be specialized without a matching request",
                        parameter.parameter
                    ),
                ));
            }
            for request in &proof.requests {
                let Some((_, actual)) = request
                    .iter()
                    .find(|(name, _)| name == &parameter.parameter)
                else {
                    failures.push(failure(
                        etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                        format!(
                            "handled request has no checked binding for `{}`",
                            parameter.parameter
                        ),
                    ));
                    continue;
                };
                let mut unifier = TypeUnifier::new(&output.store);
                if unifier.unify(*actual, parameter.candidate).is_err() {
                    failures.push(failure(etas_core::TypeDiagnosticCode::TypeMismatch, format!("handler resume type does not match every reachable request instantiation of `{}`", parameter.parameter)));
                }
            }
        }
    }
    if !failures.is_empty() {
        return Err(failures);
    }
    std::sync::Arc::make_mut(&mut output.facts)
        .deferred_handler_instantiations
        .clear();
    Ok(())
}
