use std::collections::HashMap;

use etas_hir::HirHandlerArmId;

use crate::{SolverFailure, TypeId, TypeOutput, TypeUnifier};
mod overload;

pub struct HandlerRequestProof {
    pub type_bindings: Vec<(String, TypeId)>,
    pub signature: Option<crate::ActionRequestSignature>,
}

pub struct HandlerInstantiationProof {
    pub arm: HirHandlerArmId,
    pub requests: Vec<HandlerRequestProof>,
}

pub fn complete_handler_instantiations(
    output: &mut TypeOutput,
    proofs: &[HandlerInstantiationProof],
) -> Result<(), Vec<SolverFailure>> {
    let mut indexed = HashMap::new();
    let mut failures = Vec::new();
    let mut substitutions = crate::Substitution::default();
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
        if let Some(overload) = &obligation.overload {
            match overload::solve(&output.store, overload, &proof.requests, &substitutions) {
                Ok(solved) => substitutions.extend(&solved),
                Err((code, message)) => failures.push(failure(code, message)),
            }
            continue;
        }
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
                    .type_bindings
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
    if output.facts.deferred_handler_instantiations.is_empty() {
        return Ok(());
    }
    if substitutions.iter().next().is_none() {
        std::sync::Arc::make_mut(&mut output.facts)
            .deferred_handler_instantiations
            .clear();
        return Ok(());
    }
    let mut facts = (*output.facts).clone();
    facts.deferred_handler_instantiations.clear();
    let mut interner = crate::TypeInterner::from_store((*output.store).clone());
    if let Err(message) = super::remap::substitute_facts(&mut facts, &mut interner, &substitutions)
    {
        return Err(output
            .facts
            .deferred_handler_instantiations
            .values()
            .map(|obligation| SolverFailure {
                code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                span: obligation.span,
                message: format!("handler signature completion failed: {message}"),
            })
            .collect());
    }
    output.facts = std::sync::Arc::new(facts);
    output.store = std::sync::Arc::new(interner.into_store());
    Ok(())
}
