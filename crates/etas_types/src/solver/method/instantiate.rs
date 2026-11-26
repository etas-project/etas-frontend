use super::*;
use crate::method::{MethodEvidence, MethodSelection, MethodSignatureFact};

pub(super) struct Probe {
    pub report: SolverReport,
    pub slots: Vec<u32>,
    pub defaults: Vec<crate::DefaultArgument>,
}

pub(super) fn probe(
    input: &TypeSolveInput<'_>,
    call: Input<'_>,
    candidate: &crate::CallableCandidate,
    entry: &MethodSignatureFact,
    args: &[TypeId],
    output: TypeId,
    known: &Substitution,
) -> Probe {
    let mut result = Probe {
        report: SolverReport::default(),
        slots: Vec::new(),
        defaults: Vec::new(),
    };
    if call.binding.is_none() {
        super::operation::validate(input.store, &entry.target, call, args, &mut result.report);
    }
    if !result.report.is_solved() {
        return result;
    }
    let crate::CallableCandidateOperation::Method {
        owner_params,
        evidence,
        ..
    } = &candidate.operation
    else {
        unreachable!()
    };
    let mut names = entry.parameter_names.clone();
    if call.binding.is_some() {
        names = vec![None];
    }
    if entry.receiver == crate::method::MethodReceiver::Associated {
        names.insert(0, None);
    }
    let parameters = if call.binding.is_some() {
        crate::call_interface::required_parameters(&names)
    } else {
        candidate
            .signature
            .parameter_interface
            .clone()
            .unwrap_or_else(|| crate::call_interface::required_parameters(&names))
    };
    let binding = match crate::ArgumentBinding::bind(&parameters, call.argument_names) {
        Ok(binding) => binding,
        Err(message) => {
            result.report.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::ArityMismatch,
                span: call.origin.span,
                message,
            });
            return result;
        }
    };
    let slots = binding.argument_slots;
    let mut substitutions = known.clone();
    if let MethodSelection::ExplicitSpec {
        args: selected_args,
        ..
    } = call.selection
    {
        let args = match evidence {
            MethodEvidence::Implementation { args, .. } | MethodEvidence::Bound { args, .. } => {
                args
            }
            MethodEvidence::Inherent => unreachable!(),
        };
        if selected_args.len() != args.len() {
            result.report.push(SolverFailure {
                code: etas_core::TypeDiagnosticCode::ArityMismatch,
                span: call.origin.span,
                message: "selected spec type argument arity differs".into(),
            });
            return result;
        }
        let mut unifier = TypeUnifier::with_substitution(input.store, substitutions);
        for (expected, actual) in args.iter().zip(selected_args) {
            if unifier.unify(*expected, *actual).is_err() {
                result.report.push(SolverFailure {
                    code: etas_core::TypeDiagnosticCode::TypeMismatch,
                    span: call.origin.span,
                    message: "selected spec type arguments do not match the implementation".into(),
                });
                return result;
            }
        }
        substitutions = unifier.into_substitution();
    }
    if let MethodEvidence::Bound {
        spec,
        args: expected,
        ..
    } = evidence
    {
        let receiver = super::super::resolve_substitution(input.store, &substitutions, args[0]);
        let alternatives = match spec_solver::bound_evidence::arguments(
            input.store,
            input.spec_facts,
            receiver,
            spec,
        ) {
            Ok(arguments) => arguments,
            Err(message) => {
                result.report.push(SolverFailure {
                    code: etas_core::TypeDiagnosticCode::IncompleteTypeFacts,
                    span: call.origin.span,
                    message,
                });
                return result;
            }
        };
        let mut selected = None;
        for arguments in alternatives {
            if arguments.len() != expected.len() {
                continue;
            }
            let mut unifier = TypeUnifier::with_known_substitution(input.store, &substitutions);
            if !expected
                .iter()
                .zip(arguments)
                .all(|(expected, actual)| unifier.unify(*expected, actual).is_ok())
            {
                continue;
            }
            if selected.is_some() {
                result.report.push(SolverFailure { code: etas_core::TypeDiagnosticCode::TypeMismatch, span: call.origin.span,
                    message: "method has multiple applicable declared spec bounds; select explicit spec arguments".into() });
                return result;
            }
            selected = Some(unifier.into_substitution());
        }
        if let Some(bindings) = selected {
            substitutions.extend(&bindings);
        }
    }
    result.report = solve_callable_constraint(
        input.store,
        input.spec_facts,
        CallableConstraintSolveInput {
            argument_slots: Some(&slots),
            rigid_type_parameters: input.rigid_type_parameters,
            call: Some(*call.expr),
            callee: candidate.ty,
            generic_params: &candidate.signature.generic_params,
            explicit_generic_args: call.generic_args,
            selector_bindings: &[],
            arg_exprs: call.arg_exprs,
            args,
            output,
            origin: *call.origin,
            known_substitutions: substitutions,
        },
    );
    if result.report.failures.is_empty() {
        let mut obligations = Vec::new();
        for parameter in owner_params {
            let subject =
                resolve_known_substitutions(input.store, &result.report, parameter.subject);
            for bound in &parameter.bounds {
                let obligation = SpecObligation {
                    ty: subject,
                    spec: bound.spec.clone(),
                    args: bound
                        .args
                        .iter()
                        .map(|arg| resolve_known_substitutions(input.store, &result.report, *arg))
                        .collect(),
                    span: call.origin.span,
                };
                if super::super::contains_inference_variables(input.store, subject)
                    || obligation
                        .args
                        .iter()
                        .any(|arg| super::super::contains_inference_variables(input.store, *arg))
                {
                    result.report.pending_spec_obligations.push(obligation);
                } else {
                    obligations.push(obligation);
                }
            }
        }
        if let MethodEvidence::Bound {
            spec,
            subject,
            args,
        } = evidence
        {
            obligations.push(SpecObligation {
                ty: resolve_known_substitutions(input.store, &result.report, *subject),
                spec: spec.clone(),
                args: args
                    .iter()
                    .map(|arg| resolve_known_substitutions(input.store, &result.report, *arg))
                    .collect(),
                span: call.origin.span,
            });
        }
        result.report.append(spec_solver::solve_spec_obligations(
            input.store,
            input.spec_facts,
            &obligations,
            &HashMap::new(),
        ));
    }
    result.slots = slots;
    result.defaults = binding.defaults;
    result
}
