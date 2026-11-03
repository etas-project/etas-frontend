use super::*;
use crate::method::{MethodReceiver, MethodTarget};

pub(super) fn solve<E>(
    input: &TypeSolveInput<'_>,
    call: Input<'_>,
    report: &mut SolverReport,
    access: &mut Vec<TypeConstraint>,
    checkpoint: &impl Fn() -> Result<(), E>,
) -> Result<(), E> {
    let args = call
        .args
        .iter()
        .map(|arg| resolve_known_substitutions(input.store, report, *arg))
        .collect::<Vec<_>>();
    let applicable = input
        .declaration_facts
        .methods
        .applicable(input.store, args[0], call.method);
    let mut matches = Vec::new();
    let mut failures = Vec::new();
    let mut inaccessible = None;
    for (candidate, probe) in call.candidates.iter().zip(call.probes) {
        checkpoint()?;
        if !candidates::eligible(input, call, candidate, args[0], &applicable) {
            continue;
        }
        let crate::CallableCandidateOperation::Method { index, .. } = candidate.operation else {
            unreachable!()
        };
        let entry = input.declaration_facts.methods.get(index);
        if !entry.public && entry.defining_module != *call.access_module {
            inaccessible = Some(matches!(entry.owner, crate::method::MethodOwner::Agent(_)));
            continue;
        }
        let solved = instantiate::probe(
            input,
            call,
            candidate,
            entry,
            &args,
            *probe,
            &report.substitutions,
        );
        if solved.report.is_solved() {
            matches.push(candidate);
        } else {
            failures.push(solved.report.failures);
        }
    }
    let [candidate] = matches.as_slice() else {
        if matches.len() > 1 {
            report.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::TypeMismatch,
                span: call.origin.span,
                message: format!(
                    "method `{}` is ambiguous; use `::Spec.{}` to select a spec explicitly",
                    call.method, call.method
                ),
            });
        } else if matches.is_empty() && inaccessible.is_some() && failures.is_empty() {
            report.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::TypeMismatch,
                span: call.origin.span,
                message: if inaccessible == Some(true) {
                    "agent method is private to its defining module".into()
                } else {
                    format!("method `{}` is private to its defining module", call.method)
                },
            });
        } else if failures.len() == 1 {
            report
                .failures
                .extend(failures.pop().unwrap().into_iter().map(|mut failure| {
                    failure.message = format!(
                        "{}.{}: {}",
                        display_type(input.store, args[0]),
                        call.method,
                        failure.message
                    );
                    failure
                }));
        } else {
            if let crate::method::MethodSelection::ExplicitSpec {
                spec,
                args: spec_args,
            } = call.selection
            {
                let obligation = SpecObligation {
                    ty: args[0],
                    spec: spec.clone(),
                    args: spec_args.clone(),
                    span: call.origin.span,
                };
                let evidence = spec_solver::solve_spec_obligations(
                    input.store,
                    input.spec_facts,
                    &[obligation],
                    &HashMap::new(),
                );
                if !evidence.is_solved() {
                    report.append(evidence);
                    return Ok(());
                }
            }
            report.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::TypeMismatch,
                span: call.origin.span,
                message: format!(
                    "no accessible method `{}` applies to receiver `{}` and these arguments",
                    call.method,
                    display_type(input.store, args[0])
                ),
            });
        }
        return Ok(());
    };
    let crate::CallableCandidateOperation::Method {
        index,
        owner_bindings,
        evidence,
        ..
    } = &candidate.operation
    else {
        unreachable!()
    };
    let entry = input.declaration_facts.methods.get(*index);
    let mut selected = instantiate::probe(
        input,
        call,
        candidate,
        entry,
        &args,
        resolve_known_substitutions(input.store, report, *call.output),
        &report.substitutions,
    );
    if selected.report.is_solved() {
        let bindings = match selected.report.generic_instantiations.get(call.expr) {
            Some(fact) => fact.clone(),
            None if candidate.signature.generic_params.is_empty() => {
                crate::GenericInstantiationFact {
                    type_bindings: Vec::new(),
                    effect_row_bindings: Vec::new(),
                    deferred_effect_row_obligations: Vec::new(),
                }
            }
            None => {
                selected.report.push(SolverFailure {
                    code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                    span: call.origin.span,
                    message: "selected method has no checked generic instantiation".into(),
                });
                report.append(selected.report);
                return Ok(());
            }
        };
        let component = matches!(
            entry.receiver,
            MethodReceiver::Associated | MethodReceiver::Component
        );
        let mut signature = candidate.signature.clone();
        if component {
            signature.params.remove(0);
        }
        signature.output = *call.output;
        let slots = selected.slots[1..]
            .iter()
            .map(|slot| if component { slot - 1 } else { *slot })
            .collect();
        selected.report.checked_method_calls.insert(
            *call.expr,
            crate::facts::CheckedMethodCallFact {
                target: entry.target.clone(),
                receiver: entry.receiver,
                receiver_type: args[0],
                signature,
                argument_slots: slots,
                owner_bindings: owner_bindings.clone(),
                instantiation: bindings,
                evidence: evidence.clone(),
            },
        );
        if let MethodTarget::Standard {
            operation: etas_std::FlowSourceMethodOperation::CheckedIndex,
            ..
        } = &entry.target
        {
            access.push(TypeConstraint::IndexAccess {
                expr: *call.expr,
                base: args[0],
                index: args[1],
                output: *call.output,
                index_error: input.known_std_types.index_error,
                origin: *call.origin,
            });
        }
    }
    report.append(selected.report);
    Ok(())
}
